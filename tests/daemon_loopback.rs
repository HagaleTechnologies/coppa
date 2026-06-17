//! Smoke test: verify daemon event loop can encode and decode a message
//! through the engine without CPAL (pure software path).

use coppa_engine::CoppaCore;
use coppa_protocol::mac::{Callsign, MacPdu};

#[test]
fn test_mac_pdu_roundtrip_through_engine() {
    let engine = CoppaCore::new();

    let src = Callsign::new("N0CALL").unwrap();
    let dst = Callsign::new("W1AW").unwrap();
    let payload = b"Hello from coppa!";

    let mac_pdu = MacPdu::new_data(dst.clone(), src.clone(), 0, payload.to_vec());
    let pdu_bytes = mac_pdu.to_bytes();

    // Encode to audio samples
    let samples = engine.encode_bytes(&pdu_bytes).unwrap();
    assert!(!samples.is_empty());

    // Decode back
    let decoded_bytes = engine.decode_bytes(&samples).unwrap();

    // Parse as MAC PDU
    let decoded_pdu = MacPdu::from_bytes(&decoded_bytes).unwrap();
    assert_eq!(decoded_pdu.src.as_str(), "N0CALL");
    assert_eq!(decoded_pdu.dest.as_str(), "W1AW");
    assert_eq!(decoded_pdu.payload, payload);
}

#[test]
fn test_session_handshake_roundtrip_through_engine() {
    use coppa_protocol::session::{LinkCapabilities, Session};

    let engine = CoppaCore::new();

    let local = Callsign::new("N0CALL").unwrap();
    let remote = Callsign::new("W1AW").unwrap();

    // Create initiator session and generate CONNECT_REQ
    let caps = LinkCapabilities::default();
    let mut initiator = Session::new(0, local.clone(), remote.clone(), 0, caps.clone());
    let connect_req = initiator.initiate().unwrap();
    let req_bytes = connect_req.to_bytes();

    // Encode → decode the CONNECT_REQ
    let samples = engine.encode_bytes(&req_bytes).unwrap();
    let decoded = engine.decode_bytes(&samples).unwrap();
    let decoded_pdu = MacPdu::from_bytes(&decoded).unwrap();

    // Responder handles it
    let mut responder = Session::new(0, remote.clone(), local.clone(), 0, caps);
    let connect_ack = responder.handle_connect_req(&decoded_pdu.payload).unwrap();
    let ack_bytes = connect_ack.to_bytes();

    // Encode → decode the CONNECT_ACK
    let samples2 = engine.encode_bytes(&ack_bytes).unwrap();
    let decoded2 = engine.decode_bytes(&samples2).unwrap();
    let decoded_ack = MacPdu::from_bytes(&decoded2).unwrap();

    // Initiator handles ACK
    let cfm = initiator.handle_connect_ack(&decoded_ack.payload).unwrap();
    assert!(initiator.is_established());

    // Encode → decode the CONNECT_CFM
    let samples3 = engine.encode_bytes(&cfm.to_bytes()).unwrap();
    let decoded3 = engine.decode_bytes(&samples3).unwrap();
    let decoded_cfm = MacPdu::from_bytes(&decoded3).unwrap();

    // Responder handles CFM
    responder.handle_connect_cfm(&decoded_cfm.payload).unwrap();
    assert!(responder.is_established());
}
