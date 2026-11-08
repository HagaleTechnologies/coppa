//! COP-121: dashboard chat over a session (SPEC §23, ADR-013).
//!
//! Two real `EventLoop`s, each with a real `WebSocketServer` and a
//! `tokio_tungstenite` client, exchange real modem audio. Station A
//! (VK3ABC) calls station B (W1AW).

use super::*;
use coppa_protocol::app_stream::{chat_reason, APP_STREAM, HOST_STREAM};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// One station's dashboard: its WebSocket client, the server's `HostEvent`
/// receiver (the test pumps it into the station, as `main.rs` does) and the
/// server's port, for a second client.
pub(super) struct Dashboard {
    pub(super) ws: WsClient,
    pub(super) events: mpsc::Receiver<HostEvent>,
    pub(super) port: u16,
    _task: tokio::task::JoinHandle<()>,
}

/// A WebSocket server wired to `station` the way `main.rs` wires coppad's,
/// chat log included, with one authenticated client past its `hello`.
pub(super) async fn attach_dashboard(station: &mut EventLoop) -> Dashboard {
    use coppa_host::websocket::{WebSocketServer, WsAuth};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut server =
        WebSocketServer::new(port).with_auth(WsAuth::new("test-token", Vec::<String>::new()));
    let events = server.take_event_rx().expect("fresh server");
    station.set_ws_push(server.push_handle());
    station.set_ws_broadcast(server.broadcast_sender());
    station.set_ws_status(server.status());
    station.set_ws_chat_log(server.chat_log());
    let task = tokio::spawn(async move {
        server.run_with_listener(listener).await.unwrap();
    });
    let ws = open_client(port).await;
    Dashboard {
        ws,
        events,
        port,
        _task: task,
    }
}

/// Another authenticated client on the server at `port`, past its `hello`.
pub(super) async fn open_client(port: u16) -> WsClient {
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/?token=test-token"))
            .await
            .unwrap();
    let hello = ws.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(hello.to_text().unwrap()).unwrap()["type"],
        "hello"
    );
    ws
}

pub(super) struct ChatPair {
    pub(super) a: EventLoop,
    pub(super) b: EventLoop,
    pub(super) a_out: coppa_audio::AudioRingConsumer,
    pub(super) b_out: coppa_audio::AudioRingConsumer,
    pub(super) a_dash: Dashboard,
    pub(super) b_dash: Dashboard,
}

/// COP-121: a connected pair whose dashboards were attached before the
/// handshake, with `[host] websocket_enabled = true` on both stations unless
/// a `configure` closure turns it off.
pub(super) async fn connected_chat_pair_configured(
    configure_a: impl FnOnce(&mut DaemonConfig),
    configure_b: impl FnOnce(&mut DaemonConfig),
) -> ChatPair {
    let (mut a, mut b, mut a_out, mut b_out) = unconnected_pair(
        0,
        |c| {
            c.host.websocket_enabled = true;
            configure_a(c);
        },
        |c| {
            c.host.websocket_enabled = true;
            configure_b(c);
        },
    );
    let a_dash = attach_dashboard(&mut a).await;
    let b_dash = attach_dashboard(&mut b).await;
    handshake_pair(&mut a, &mut b, &mut a_out, &mut b_out).await;
    ChatPair {
        a,
        b,
        a_out,
        b_out,
        a_dash,
        b_dash,
    }
}

pub(super) async fn connected_chat_pair() -> ChatPair {
    connected_chat_pair_configured(|_| {}, |_| {}).await
}

pub(super) async fn send_json(ws: &mut WsClient, json: &str) {
    ws.send(Message::Text(json.into())).await.unwrap();
}

/// The next message whose `type` is one of `kinds`, skipping others.
pub(super) async fn next_of(ws: &mut WsClient, kinds: &[&str]) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            let Ok(text) = message.to_text() else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_str(text).unwrap();
            if kinds.iter().any(|k| value["type"] == *k) {
                return value;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kinds:?} message within 5 s"))
}

/// Every message that arrives within `ms`.
pub(super) async fn drain_ws(ws: &mut WsClient, ms: u64) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(Some(Ok(message))) =
        tokio::time::timeout(Duration::from_millis(ms), ws.next()).await
    {
        if let Ok(text) = message.to_text() {
            out.push(serde_json::from_str(text).unwrap());
        }
    }
    out
}

/// Send a WebSocket `chat` request from `dash` and return the reply (`ack`
/// or `error`). On `ack`, the `ChatSend` event the server produced is fed to
/// `station`, as coppad's host-event loop would.
pub(super) async fn request_chat(
    dash: &mut Dashboard,
    station: &mut EventLoop,
    text: &str,
    id: u64,
) -> serde_json::Value {
    let request = serde_json::json!({"type": "chat", "text": text, "id": id});
    send_json(&mut dash.ws, &request.to_string()).await;
    let reply = next_of(&mut dash.ws, &["ack", "error"]).await;
    assert_eq!(reply["id"], id, "{reply}");
    if reply["type"] == "ack" {
        // Client `Connected`/`Disconnected` events come through the same
        // channel; forward everything until the chat itself.
        loop {
            let event = tokio::time::timeout(Duration::from_secs(3), dash.events.recv())
                .await
                .expect("the server forwards an accepted chat")
                .expect("event channel open");
            let is_chat = matches!(&event, HostEvent::ChatSend { text: t, .. } if t == text);
            station.handle_host_event(event).await;
            if is_chat {
                break;
            }
        }
    }
    reply
}

/// Samples the station has put out since the last read; empty when it
/// transmitted nothing.
pub(super) fn transmitted(consumer: &mut coppa_audio::AudioRingConsumer) -> usize {
    let mut buf = vec![0.0f32; 1_000_000];
    consumer.read(&mut buf)
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The transport segment of a session data frame `station` would decode.
pub(super) fn segment_of(frame: &[u8]) -> TransportPdu {
    let mac = MacPdu::from_bytes(frame).unwrap();
    TransportPdu::from_bytes(&mac.payload).unwrap()
}

/// Gherkin: an operator sends a message; the peer receives it and the
/// sender sees a delivery receipt.
#[tokio::test]
async fn chat_reaches_the_peer_and_the_sender_sees_a_receipt() {
    let mut p = connected_chat_pair().await;

    let reply = request_chat(&mut p.a_dash, &mut p.a, "Good morning W1AW", 1).await;
    assert_eq!(reply["type"], "ack");
    let sent = next_of(&mut p.a_dash.ws, &["chat"]).await;
    assert_eq!(sent["direction"], "out");
    assert_eq!(sent["state"], "sending");
    assert_eq!(sent["text"], "Good morning W1AW");
    assert_eq!(sent["remote_call"], "W1AW");
    let message_id = sent["message_id"].as_u64().unwrap();
    assert_eq!(
        p.a.session_link.as_ref().unwrap().tx_stream,
        APP_STREAM,
        "the chat segment is in flight on stream 1"
    );

    unstick_decoder(&mut p.b);
    p.b.decode_and_dispatch_audio(&take_leg(&mut p.a_out, "the chat frame"))
        .await;
    let received = next_of(&mut p.b_dash.ws, &["chat", "data"]).await;
    assert_eq!(received["type"], "chat", "B must not see chat as data");
    assert_eq!(received["direction"], "in");
    assert_eq!(received["state"], "received");
    assert_eq!(received["text"], "Good morning W1AW");
    assert_eq!(received["remote_call"], "VK3ABC");
    assert!(drain_ws(&mut p.b_dash.ws, 200)
        .await
        .iter()
        .all(|m| m["type"] != "data" && m["type"] != "chat"));

    unstick_decoder(&mut p.a);
    p.a.decode_and_dispatch_audio(&take_leg(&mut p.b_out, "B's ACK"))
        .await;
    let receipt = next_of(&mut p.a_dash.ws, &["chat_receipt"]).await;
    assert_eq!(receipt["message_id"], message_id);
    assert_eq!(receipt["state"], "delivered");
    assert!(receipt.get("reason").is_none());

    send_json(&mut p.a_dash.ws, r#"{"type":"chat_history","id":2}"#).await;
    let history = next_of(&mut p.a_dash.ws, &["chat_history"]).await;
    assert_eq!(history["id"], 2);
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["message_id"], message_id);
    assert_eq!(messages[0]["state"], "delivered");
}

#[tokio::test]
async fn chat_is_negotiated_only_when_both_stations_enable_the_websocket_api() {
    let mut on = DaemonConfig::default();
    on.host.websocket_enabled = true;
    assert_eq!(hex(&local_link_caps(&on).to_bytes()), "00c80102081600");
    on.session.fade_protection = crate::config::FadeProtection::Off;
    assert_eq!(hex(&local_link_caps(&on).to_bytes()), "00c80102081200");

    let mut p = connected_chat_pair_configured(|_| {}, |c| c.host.websocket_enabled = false).await;
    for station in [&p.a, &p.b] {
        let caps = station
            .session_mgr
            .get(station.session_link.as_ref().unwrap().session_id)
            .and_then(|s| s.negotiated.clone())
            .unwrap();
        assert!(!caps.app_messages);
    }
    p.a.publish_ws_snapshot().await;
    send_json(&mut p.a_dash.ws, r#"{"type":"status","id":3}"#).await;
    let status = next_of(&mut p.a_dash.ws, &["status"]).await;
    assert_eq!(status["chat"], false, "{status}");

    let reply = request_chat(&mut p.a_dash, &mut p.a, "hello", 4).await;
    assert_eq!(reply["type"], "error");
    assert_eq!(reply["message"], chat_reason::PEER_WITHOUT_CHAT);
    assert_eq!(transmitted(&mut p.a_out), 0, "nothing is transmitted");
}

#[tokio::test]
async fn daemon_refuses_chat_it_cannot_send() {
    // No session at all.
    let (mut lone, _, mut lone_out, _) =
        unconnected_pair(0, |c| c.host.websocket_enabled = true, |_| {});
    let mut dash = attach_dashboard(&mut lone).await;
    lone.handle_host_event(HostEvent::ChatSend {
        client_id: 1,
        text: "hello".into(),
    })
    .await;
    let refused = next_of(&mut dash.ws, &["chat"]).await;
    assert_eq!(refused["state"], "failed");
    assert_eq!(refused["reason"], chat_reason::NO_SESSION);
    assert_eq!(transmitted(&mut lone_out), 0);

    // A session that already has 32 messages waiting.
    let mut p = connected_chat_pair().await;
    {
        let link = p.a.session_link.as_mut().unwrap();
        for id in 1000..1032 {
            link.chat_tx.queue(id, 8);
        }
    }
    p.a.handle_host_event(HostEvent::ChatSend {
        client_id: 1,
        text: "one too many".into(),
    })
    .await;
    let refused = next_of(&mut p.a_dash.ws, &["chat"]).await;
    assert_eq!(refused["state"], "failed");
    assert_eq!(refused["reason"], chat_reason::TOO_MANY_WAITING);
    assert!(p.a.session_link.as_ref().unwrap().app_backlog.is_empty());
    assert_eq!(transmitted(&mut p.a_out), 0);

    // A session draining for a host DISCONNECT.
    let mut p = connected_chat_pair().await;
    p.a.handle_host_event(HostEvent::DataReceived {
        client_id: 1,
        data: b"last words".to_vec(),
    })
    .await;
    take_leg(&mut p.a_out, "the host data frame");
    p.a.handle_ptt_change(false).await;
    p.a.handle_host_event(HostEvent::DisconnectRequest { client_id: 1 })
        .await;
    assert!(p.a.pending_disconnect.is_some(), "the drain is pending");
    p.a.handle_host_event(HostEvent::ChatSend {
        client_id: 1,
        text: "too late".into(),
    })
    .await;
    let refused = next_of(&mut p.a_dash.ws, &["chat"]).await;
    assert_eq!(refused["state"], "failed");
    assert_eq!(refused["reason"], chat_reason::SESSION_CLOSING);
    assert_eq!(transmitted(&mut p.a_out), 0);
}

#[tokio::test]
async fn chat_bytes_are_not_buffer_bytes() {
    let mut p = connected_chat_pair().await;
    p.a.publish_ws_snapshot().await;
    let before = p.a.buffered_tx_bytes();
    assert_eq!(before, 0);
    let reply = request_chat(&mut p.a_dash, &mut p.a, "not in BUFFER", 1).await;
    assert_eq!(reply["type"], "ack");
    let link = p.a.session_link.as_ref().unwrap();
    assert_eq!(link.tx_stream, APP_STREAM);
    assert!(link.tx.in_flight() == 1, "the chat segment is in flight");
    assert_eq!(p.a.buffered_tx_bytes(), 0);
    send_json(&mut p.a_dash.ws, r#"{"type":"status","id":5}"#).await;
    let status = next_of(&mut p.a_dash.ws, &["status"]).await;
    assert_eq!(status["buffer"], 0, "{status}");
    assert_eq!(status["chat"], true, "{status}");
}

#[tokio::test]
async fn pending_chat_fails_when_the_peer_disconnects() {
    let mut p = connected_chat_pair().await;
    let reply = request_chat(&mut p.a_dash, &mut p.a, "are you there?", 1).await;
    assert_eq!(reply["type"], "ack");
    let sent = next_of(&mut p.a_dash.ws, &["chat"]).await;
    let message_id = sent["message_id"].as_u64().unwrap();
    take_leg(&mut p.a_out, "the chat frame, which B never hears");
    p.a.handle_ptt_change(false).await;

    p.b.handle_host_event(HostEvent::DisconnectRequest { client_id: 1 })
        .await;
    unstick_decoder(&mut p.a);
    p.a.decode_and_dispatch_audio(&take_leg(&mut p.b_out, "B's DISCONNECT"))
        .await;
    let receipt = next_of(&mut p.a_dash.ws, &["chat_receipt"]).await;
    assert_eq!(receipt["message_id"], message_id);
    assert_eq!(receipt["state"], "failed");
    assert_eq!(receipt["reason"], chat_reason::SESSION_ENDED);

    send_json(&mut p.a_dash.ws, r#"{"type":"status","id":6}"#).await;
    let status = next_of(&mut p.a_dash.ws, &["status"]).await;
    assert!(status.get("chat").is_none(), "{status}");
}

/// Today's rule, kept: without `app_messages` the stream nibble is ignored,
/// so a nibble-1 segment is host data and is ACKed.
#[tokio::test]
async fn a_stream_1_segment_is_host_data_without_app_messages() {
    let (mut a, mut b, _a_out, mut b_out) = connected_pair(0).await;
    let mut dash = attach_dashboard(&mut b).await;
    assert!(!b.session_app_messages());
    let frame = session_stream_frame(a.session_link.as_ref().unwrap(), APP_STREAM, 0, b"chat?");
    assert_eq!(segment_of(&frame).session_id, APP_STREAM);
    let pdu = MacPdu::from_bytes(&frame).unwrap();
    b.handle_mac_pdu(pdu).await;
    let data = next_of(&mut dash.ws, &["data", "chat"]).await;
    assert_eq!(data["type"], "data");
    assert_eq!(data["data"], "chat?");
    assert!(transmitted(&mut b_out) > 0, "B ACKed the segment");
    assert_eq!(b.session_link.as_ref().unwrap().rx.recv_base(), 1);
    a.handle_ptt_change(false).await;
}

/// Host bytes keep today's frames byte for byte on a chat session.
#[tokio::test]
async fn host_bytes_keep_stream_0_on_a_chat_session() {
    let mut p = connected_chat_pair().await;
    p.a.handle_host_event(HostEvent::DataReceived {
        client_id: 1,
        data: b"HELLO".to_vec(),
    })
    .await;
    let link = p.a.session_link.as_ref().unwrap();
    assert_eq!(link.tx_stream, HOST_STREAM);
    let frame = session_stream_frame(
        link,
        link.tx_stream,
        0,
        link.tx.get_segment_data(0).unwrap(),
    );
    assert_eq!(frame, session_data_frame(link, 0, b"HELLO"));
    assert_eq!(segment_of(&frame).session_id, HOST_STREAM);
}

/// A peer's chat record over the 512-byte limit is ACKed and skipped:
/// nothing reaches the dashboard or the history.
#[tokio::test]
async fn an_oversized_incoming_chat_record_is_skipped() {
    let mut p = connected_chat_pair().await;
    let record =
        app_stream::encode_record(&coppa_protocol::app::AppPdu::new_text(&[b'x'; 600])).unwrap();
    let frame = session_stream_frame(p.a.session_link.as_ref().unwrap(), APP_STREAM, 0, &record);
    p.b.handle_mac_pdu(MacPdu::from_bytes(&frame).unwrap())
        .await;
    assert!(transmitted(&mut p.b_out) > 0, "B ACKed the segment");
    assert!(drain_ws(&mut p.b_dash.ws, 200)
        .await
        .iter()
        .all(|m| m["type"] != "chat" && m["type"] != "data"));
    send_json(&mut p.b_dash.ws, r#"{"type":"chat_history","id":9}"#).await;
    let history = next_of(&mut p.b_dash.ws, &["chat_history"]).await;
    assert!(history["messages"].as_array().unwrap().is_empty());
}

mod more;
