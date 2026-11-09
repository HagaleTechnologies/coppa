//! COP-121: chat across segments, alongside host bytes, through drains,
//! reserved streams, retransmissions, outer-code bursts and reconnects.

use super::*;

/// Pseudo-random letters: a chat text that does not compress to nothing.
fn letters(len: usize, seed: u32) -> String {
    let mut seed = seed;
    (0..len)
        .map(|_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            char::from(b'a' + ((seed >> 16) % 26) as u8)
        })
        .collect()
}

/// One stop-and-wait round: A's frame reaches B, B's ACK reaches A. Returns
/// false when A had nothing in flight.
async fn relay_round(p: &mut ChatPair) -> bool {
    if p.a.session_link.as_ref().unwrap().tx.in_flight() == 0 {
        return false;
    }
    let frame = take_leg(&mut p.a_out, "A's session frame");
    p.a.handle_ptt_change(false).await;
    unstick_decoder(&mut p.b);
    p.b.decode_and_dispatch_audio(&frame).await;
    let ack = take_leg(&mut p.b_out, "B's ACK");
    p.b.handle_ptt_change(false).await;
    unstick_decoder(&mut p.a);
    p.a.decode_and_dispatch_audio(&ack).await;
    true
}

#[tokio::test]
async fn chat_spanning_two_segments_is_receipted_after_the_last_ack() {
    let one_codeword = |c: &mut DaemonConfig| c.session.max_codewords_per_frame = 1;
    let mut p = connected_chat_pair_configured(one_codeword, one_codeword).await;
    let text = letters(200, 0xC0_12_10_04);
    let reply = request_chat(&mut p.a_dash, &mut p.a, &text, 1).await;
    assert_eq!(reply["type"], "ack");
    let sent = next_of(&mut p.a_dash.ws, &["chat"]).await;
    let message_id = sent["message_id"].as_u64().unwrap();

    let mut segments = 0;
    let mut receipts = Vec::new();
    let mut b_chats = Vec::new();
    while relay_round(&mut p).await {
        segments += 1;
        assert!(segments < 50, "the message must finish");
        let a_msgs = drain_ws(&mut p.a_dash.ws, 100).await;
        let b_msgs = drain_ws(&mut p.b_dash.ws, 100).await;
        let link = p.a.session_link.as_ref().unwrap();
        let last = link.tx.in_flight() == 0 && link.app_backlog.is_empty();
        let round_receipts: Vec<_> = a_msgs
            .into_iter()
            .filter(|m| m["type"] == "chat_receipt")
            .collect();
        let round_chats: Vec<_> = b_msgs
            .into_iter()
            .filter(|m| m["type"] == "chat" || m["type"] == "data")
            .collect();
        if !last {
            assert!(
                round_receipts.is_empty(),
                "no receipt before the last ACK (segment {segments})"
            );
            assert!(
                round_chats.is_empty(),
                "B shows nothing before the last segment"
            );
        }
        receipts.extend(round_receipts);
        b_chats.extend(round_chats);
    }
    assert!(
        segments >= 2,
        "200 bytes at one codeword need several segments"
    );
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    assert_eq!(receipts[0]["message_id"], message_id);
    assert_eq!(receipts[0]["state"], "delivered");
    assert_eq!(b_chats.len(), 1, "{b_chats:?}");
    assert_eq!(b_chats[0]["type"], "chat");
    assert_eq!(b_chats[0]["text"], text.as_str());
}

#[tokio::test]
async fn chat_goes_ahead_of_queued_host_bytes() {
    let mut p = connected_chat_pair().await;
    let (data_tx, mut data_rx) = mpsc::unbounded_channel();
    p.b.set_response_tx(data_tx);
    let host = letters(2_000, 0xC0_12_10_05).into_bytes();
    p.a.handle_host_event(HostEvent::DataReceived {
        client_id: 1,
        data: host.clone(),
    })
    .await;
    {
        let link = p.a.session_link.as_ref().unwrap();
        assert_eq!(link.tx_stream, HOST_STREAM);
        assert!(!link.backlog.is_empty(), "host bytes stay queued");
    }
    let reply = request_chat(&mut p.a_dash, &mut p.a, "QSY to 3.590?", 1).await;
    assert_eq!(reply["type"], "ack");
    assert!(
        !p.a.session_link.as_ref().unwrap().app_backlog.is_empty(),
        "the chat waits behind the in-flight host segment"
    );

    assert!(relay_round(&mut p).await);
    let link = p.a.session_link.as_ref().unwrap();
    assert_eq!(link.tx.in_flight(), 1);
    assert_eq!(link.tx_stream, APP_STREAM, "the next segment is the chat");
    let seq = link.tx.send_base();
    let frame = session_stream_frame(
        link,
        link.tx_stream,
        seq,
        link.tx.get_segment_data(seq).unwrap(),
    );
    assert_eq!(segment_of(&frame).session_id, APP_STREAM);
    assert!(link.app_backlog.is_empty());
    assert!(!link.backlog.is_empty(), "host bytes still wait");

    let mut b_order = Vec::new();
    let mut rounds = 0;
    while relay_round(&mut p).await {
        rounds += 1;
        assert!(rounds < 100, "the transfer must finish");
        for m in drain_ws(&mut p.b_dash.ws, 50).await {
            if m["type"] == "chat" || m["type"] == "data" {
                b_order.push(m["type"].as_str().unwrap().to_string());
            }
        }
    }
    let chat_at = b_order
        .iter()
        .position(|t| t == "chat")
        .expect("B saw the chat");
    let last_data = b_order
        .iter()
        .rposition(|t| t == "data")
        .expect("B saw host data after the chat");
    assert!(chat_at < last_data, "{b_order:?}");
    assert_eq!(drain_data_out(&mut data_rx).concat(), host);
    let receipt = next_of(&mut p.a_dash.ws, &["chat_receipt"]).await;
    assert_eq!(receipt["state"], "delivered");
}

#[tokio::test]
async fn chat_queued_before_a_local_disconnect_is_delivered_first() {
    let mut p = connected_chat_pair().await;
    let reply = request_chat(&mut p.a_dash, &mut p.a, "73", 1).await;
    assert_eq!(reply["type"], "ack");
    let sent = next_of(&mut p.a_dash.ws, &["chat"]).await;
    let message_id = sent["message_id"].as_u64().unwrap();
    let frame = take_leg(&mut p.a_out, "the chat frame");
    p.a.handle_ptt_change(false).await;

    send_json(&mut p.a_dash.ws, r#"{"type":"disconnect","id":2}"#).await;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(3), p.a_dash.events.recv())
            .await
            .expect("the server forwards the disconnect")
            .expect("event channel open");
        let is_disconnect = matches!(event, HostEvent::DisconnectRequest { .. });
        p.a.handle_host_event(event).await;
        if is_disconnect {
            break;
        }
    }
    assert!(
        p.a.pending_disconnect.is_some(),
        "the drain waits for the chat"
    );
    assert_eq!(
        transmitted(&mut p.a_out),
        0,
        "no DISCONNECT before the chat's ACK"
    );

    unstick_decoder(&mut p.b);
    p.b.decode_and_dispatch_audio(&frame).await;
    let ack = take_leg(&mut p.b_out, "B's ACK");
    p.b.handle_ptt_change(false).await;
    unstick_decoder(&mut p.a);
    p.a.decode_and_dispatch_audio(&ack).await;
    let receipt = next_of(&mut p.a_dash.ws, &["chat_receipt"]).await;
    assert_eq!(receipt["message_id"], message_id);
    assert_eq!(receipt["state"], "delivered");
    assert!(
        transmitted(&mut p.a_out) > 0,
        "the DISCONNECT goes out once the chat is ACKed"
    );
}

#[tokio::test]
async fn an_unknown_stream_segment_is_acked_and_dropped() {
    let mut p = connected_chat_pair().await;
    assert!(p.b.session_app_messages());
    let reserved = {
        let link = p.a.session_link.as_ref().unwrap();
        session_stream_frame(link, 2, 0, b"reserved")
    };
    assert_eq!(segment_of(&reserved).session_id, 2);
    p.b.handle_mac_pdu(MacPdu::from_bytes(&reserved).unwrap())
        .await;
    assert!(
        transmitted(&mut p.b_out) > 0,
        "B ACKed the reserved segment"
    );
    p.b.handle_ptt_change(false).await;
    assert_eq!(p.b.session_link.as_ref().unwrap().rx.recv_base(), 1);
    assert!(p.b.session_link.as_ref().unwrap().warned_unknown_stream);
    assert!(drain_ws(&mut p.b_dash.ws, 200)
        .await
        .iter()
        .all(|m| m["type"] != "data" && m["type"] != "chat"));

    let host = {
        let link = p.a.session_link.as_ref().unwrap();
        session_stream_frame(link, HOST_STREAM, 1, b"after")
    };
    p.b.handle_mac_pdu(MacPdu::from_bytes(&host).unwrap()).await;
    let data = next_of(&mut p.b_dash.ws, &["data", "chat"]).await;
    assert_eq!(data["type"], "data");
    assert_eq!(data["data"], "after");
}

#[tokio::test]
async fn a_retransmitted_chat_segment_is_byte_identical() {
    let mut p = connected_chat_pair().await;
    fast_session_rto(&mut p.a);
    let reply = request_chat(&mut p.a_dash, &mut p.a, "say again", 1).await;
    assert_eq!(reply["type"], "ack");
    let sent = next_of(&mut p.a_dash.ws, &["chat"]).await;
    let message_id = sent["message_id"].as_u64().unwrap();
    let first_frame = {
        let link = p.a.session_link.as_ref().unwrap();
        assert_eq!(link.tx_stream, APP_STREAM);
        session_stream_frame(link, APP_STREAM, 0, link.tx.get_segment_data(0).unwrap())
    };
    let first = take_leg(&mut p.a_out, "the chat frame, which is lost");
    p.a.handle_ptt_change(false).await;
    sleep_past_session_rto(&p.a);
    p.a.check_session_retransmits().await;
    let second = take_leg(&mut p.a_out, "the retransmission");
    p.a.handle_ptt_change(false).await;
    assert_eq!(first, second, "the retransmission is byte-identical");
    {
        let link = p.a.session_link.as_ref().unwrap();
        assert_eq!(link.tx_stream, APP_STREAM);
        let rebuilt = session_stream_frame(
            link,
            link.tx_stream,
            0,
            link.tx.get_segment_data(0).unwrap(),
        );
        assert_eq!(rebuilt, first_frame);
        assert_eq!(segment_of(&rebuilt).session_id, APP_STREAM);
    }
    assert!(drain_ws(&mut p.a_dash.ws, 100)
        .await
        .iter()
        .all(|m| m["type"] != "chat_receipt"));

    unstick_decoder(&mut p.b);
    p.b.decode_and_dispatch_audio(&second).await;
    let received = next_of(&mut p.b_dash.ws, &["chat", "data"]).await;
    assert_eq!(received["type"], "chat");
    assert_eq!(received["text"], "say again");
    let ack = take_leg(&mut p.b_out, "B's ACK");
    p.b.handle_ptt_change(false).await;
    unstick_decoder(&mut p.a);
    p.a.decode_and_dispatch_audio(&ack).await;
    let receipt = next_of(&mut p.a_dash.ws, &["chat_receipt"]).await;
    assert_eq!(receipt["message_id"], message_id);
    assert_eq!(receipt["state"], "delivered");
}

#[tokio::test]
async fn chat_rides_an_rs12_6_outer_block() {
    let request = |c: &mut DaemonConfig| {
        c.session.fade_protection = crate::config::FadeProtection::Request;
    };
    let mut p = connected_chat_pair_configured(request, request).await;
    assert_eq!(p.a.session_fade_mode(), "rs12-6");
    p.a.engine.set_speed_level(2).unwrap();
    p.b.engine.set_speed_level(2).unwrap();
    let reply = request_chat(&mut p.a_dash, &mut p.a, "73 via RS", 1).await;
    assert_eq!(reply["type"], "ack");
    let sent = next_of(&mut p.a_dash.ws, &["chat"]).await;
    let message_id = sent["message_id"].as_u64().unwrap();
    {
        let link = p.a.session_link.as_ref().unwrap();
        assert!(
            link.tx_outer.is_some(),
            "the chat went out as an outer burst"
        );
        assert_eq!(link.tx_stream, APP_STREAM);
    }
    let burst = take_leg(&mut p.a_out, "the RS(12,6) burst");
    p.a.handle_ptt_change(false).await;
    unstick_decoder(&mut p.b);
    p.b.decode_and_dispatch_audio(&burst).await;
    let received = next_of(&mut p.b_dash.ws, &["chat", "data"]).await;
    assert_eq!(
        received["type"], "chat",
        "the rebuilt block is chat, not data"
    );
    assert_eq!(received["text"], "73 via RS");

    // The block ACK waits for the burst's airtime; release it now.
    p.b.session_link.as_mut().unwrap().block_ack_at = Some(clock_now());
    p.b.flush_block_ack().await;
    let ack = take_leg(&mut p.b_out, "B's doubled block ACK");
    p.b.handle_ptt_change(false).await;
    unstick_decoder(&mut p.a);
    p.a.decode_and_dispatch_audio(&ack).await;
    let receipt = next_of(&mut p.a_dash.ws, &["chat_receipt"]).await;
    assert_eq!(receipt["message_id"], message_id);
    assert_eq!(receipt["state"], "delivered");
}

#[tokio::test]
async fn chat_history_after_a_reconnect_shows_final_states() {
    let mut p = connected_chat_pair().await;
    let reply = request_chat(&mut p.a_dash, &mut p.a, "Good morning", 1).await;
    assert_eq!(reply["type"], "ack");
    let sent = next_of(&mut p.a_dash.ws, &["chat"]).await;
    let message_id = sent["message_id"].as_u64().unwrap();
    assert!(relay_round(&mut p).await);
    let receipt = next_of(&mut p.a_dash.ws, &["chat_receipt"]).await;
    assert_eq!(receipt["state"], "delivered");

    let mut again = open_client(p.a_dash.port).await;
    send_json(&mut again, r#"{"type":"chat_history","id":9}"#).await;
    let history = next_of(&mut again, &["chat_history"]).await;
    assert_eq!(history["id"], 9);
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{history}");
    assert_eq!(messages[0]["message_id"], message_id);
    assert_eq!(messages[0]["direction"], "out");
    assert_eq!(messages[0]["state"], "delivered");
    assert_eq!(messages[0]["text"], "Good morning");

    let mut b_again = open_client(p.b_dash.port).await;
    send_json(&mut b_again, r#"{"type":"chat_history"}"#).await;
    let history = next_of(&mut b_again, &["chat_history"]).await;
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{history}");
    assert_eq!(messages[0]["direction"], "in");
    assert_eq!(messages[0]["state"], "received");
    assert_eq!(messages[0]["remote_call"], "VK3ABC");
}
