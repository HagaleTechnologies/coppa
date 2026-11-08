//! Frozen application-message vectors (COP-121, SPEC.md §23).
//!
//! Every hex string below is printed in SPEC.md §23, and
//! `spec_23_rows_carry_the_frozen_hex` fails if a §23 row is deleted, renamed
//! or carries different bytes. `a01_to_a04_come_from_the_public_encoders`
//! rebuilds each row from the public `coppa_protocol` API. The A02 session
//! `Data` frame mirrors coppad's `session_data_frame` with stream 1, as the
//! §21 rows in `protocol_vectors.rs` do for stream 0.

use coppa_protocol::app::{AppProtocol, Compression, FragFlags};
use coppa_protocol::app_stream::{chat_record, RecordReader, APP_STREAM};
use coppa_protocol::mac::{Callsign, MacPdu};
use coppa_protocol::session::LinkCapabilities;
use coppa_protocol::transport::{TransportPdu, TransportType};

/// Every §23 row: (row ID, frozen lowercase hex).
const VECTORS: &[(&str, &str)] = &[
    ("A01", "0006000000013733"),
    (
        "A02",
        "105dc05700000058b78108300000110000000000000006000000013733",
    ),
    ("A03", "00c80102081600"),
    ("A04", "00c80102080600"),
];

const SPEC: &str = include_str!("../../../docs/SPEC.md");

fn vector(id: &str) -> &'static str {
    VECTORS
        .iter()
        .find(|(row, _)| *row == id)
        .map(|(_, hex)| *hex)
        .unwrap_or_else(|| panic!("no vector {id}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn call(s: &str) -> Callsign {
    Callsign::new(s).expect("valid callsign")
}

/// coppad 0.11.0 with its WebSocket API on, fade protection `accept`.
fn coppad_chat_caps() -> LinkCapabilities {
    LinkCapabilities {
        multi_frame_ack: true,
        outer_code_rx: true,
        app_messages: true,
        ..LinkCapabilities::default()
    }
}

#[test]
fn a01_to_a04_come_from_the_public_encoders() {
    // A01: the chat record for `73`.
    let record = chat_record("73").expect("valid chat text");
    assert_eq!(hex(&record), vector("A01"));

    // A02: a session `Data` frame, seq 0, stream 1, VK3ABC to W1AW.
    let segment = TransportPdu::new_reliable(APP_STREAM, 0, 0, record.clone());
    let frame = MacPdu::new_data(call("W1AW"), call("VK3ABC"), 0, segment.to_bytes()).to_bytes();
    assert_eq!(hex(&frame), vector("A02"));

    // Decode A02 back: the transport nibble is the stream, the payload is A01.
    let mac = MacPdu::from_bytes(&frame).expect("A02 is a MAC PDU");
    assert_eq!(mac.dest, call("W1AW"));
    assert_eq!(mac.src, call("VK3ABC"));
    let transport = TransportPdu::from_bytes(&mac.payload).expect("A02 carries a transport PDU");
    assert_eq!(transport.transport_type, TransportType::Reliable);
    assert_eq!(transport.session_id, APP_STREAM);
    assert_eq!(transport.seq_num, 0);
    assert_eq!(transport.payload, record);
    let mut reader = RecordReader::default();
    let pdus = reader.push(&transport.payload);
    assert_eq!(pdus.len(), 1);
    let pdu = pdus[0].as_ref().expect("A01 is a valid record");
    assert_eq!(pdu.protocol, AppProtocol::Text);
    assert_eq!(pdu.compression, Compression::None);
    assert_eq!(pdu.frag_flags, FragFlags::Complete);
    assert_eq!(pdu.frag_total, 1);
    assert_eq!(pdu.payload, b"73");

    // A03: coppad 0.11.0, WebSocket API on, fade protection `accept`.
    let a03 = coppad_chat_caps();
    assert_eq!(hex(&a03.to_bytes()), vector("A03"));
    assert!(
        LinkCapabilities::from_bytes(&a03.to_bytes())
            .unwrap()
            .app_messages
    );

    // A04: A03 negotiated with a coppad 0.10.0 at `accept`: chat off, and the
    // bytes equal today's default coppad capabilities.
    let old = LinkCapabilities {
        multi_frame_ack: true,
        outer_code_rx: true,
        ..LinkCapabilities::default()
    };
    let a04 = a03.negotiate(&old);
    assert!(!a04.app_messages);
    assert_eq!(hex(&a04.to_bytes()), vector("A04"));
    assert_eq!(a04, old.negotiate(&a03));
}

// ── Binding to SPEC.md §23 ──

fn spec_section_23() -> &'static str {
    // No trailing `\n` in the pattern, so a CRLF checkout still matches.
    let start = SPEC
        .find("\n## 23. Application messages on a session")
        .expect("SPEC.md has a `## 23. Application messages on a session` heading");
    let body = &SPEC[start + 1..];
    match body[3..].find("\n## ") {
        Some(end) => &body[..end + 3],
        None => body,
    }
}

/// Every vector has exactly one §23 table row starting `| <ID> |`, that row
/// carries the frozen hex in backticks, and every §23 row ID has a vector.
#[test]
fn spec_23_rows_carry_the_frozen_hex() {
    let section = spec_section_23();
    for (id, expected) in VECTORS {
        let prefix = format!("| {id} |");
        let rows: Vec<&str> = section
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(&prefix))
            .collect();
        assert_eq!(rows.len(), 1, "SPEC.md §23 must have one row for {id}");
        assert!(
            rows[0].contains(&format!("`{expected}`")),
            "SPEC.md §23 row {id} does not carry `{expected}`: {}",
            rows[0]
        );
    }
    for line in section.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("| ") else {
            continue;
        };
        let id = rest.split(" |").next().unwrap_or("");
        let is_row_id =
            id.len() == 3 && id.starts_with('A') && id[1..].bytes().all(|b| b.is_ascii_digit());
        if is_row_id {
            assert!(
                VECTORS.iter().any(|(v, _)| *v == id),
                "SPEC.md §23 row {id} has no frozen vector in this test"
            );
        }
    }
}
