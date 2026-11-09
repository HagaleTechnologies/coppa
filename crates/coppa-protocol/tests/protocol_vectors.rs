//! Frozen upper-layer protocol vectors (COP-130, SPEC.md §21).
//!
//! Locks the exact bytes of the MAC (§17), transport (§18), ARQ (§19) and
//! session (§20) wire formats for one coherent two-station conversation
//! (VK3ABC calls W1AW) plus focused transport, capability, CP-control, PHY
//! header and §15 envelope rows. These are documentation vectors first and
//! tests second: every hex string below is printed in SPEC.md §21, and
//! `spec_21_rows_match_frozen_vectors` fails if a §21 row is deleted, renamed
//! or carries different bytes. Regenerate both together, deliberately, if
//! either ever changes.
//!
//! Each row is checked three ways:
//! 1. the public encoders (and session/ARQ transitions) produce the literal;
//! 2. a small test-only decoder that splits the bytes at the offsets SPEC.md
//!    §17/§18/§20 publish (not `MacPdu::from_bytes`) recovers every field,
//!    which is the executable form of "decode the vectors by hand";
//! 3. the literal is bound to its §21 table row by ID.
//!
//! Expected outputs were reproduced against the public `coppa_protocol` API
//! at commit edb17bc. Session Data/Ack frames mirror coppad's
//! `session_data_frame`/`session_ack_frame` (`coppa-daemon` `event_loop.rs`),
//! and the beacon mirrors `build_beacon_mac_pdu`; the daemon crate is not a
//! dependency of this one, so those three builders are restated here.

// The session and ARQ walks need one arbitrary start instant; nothing here
// depends on wall-clock time (COP-90's clock rule is for production code).
#![allow(clippy::disallowed_methods)]

use std::time::{Duration, Instant};

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::compression::{lz4_compress, lz4_decompress, HuffmanCodec};
use coppa_protocol::cp_negotiator::{ContentAction, CpMode, CpNegotiator};
use coppa_protocol::error::ProtocolError;
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu, StationIdPayload};
use coppa_protocol::session::{LinkCapabilities, Session, SessionState};
use coppa_protocol::transport::{TransportPdu, TransportType};

/// Every §21 row: (row ID, frozen lowercase hex).
const VECTORS: &[(&str, &str)] = &[
    ("P01", "125dc05700000058b7810830000000c80102080200"),
    ("P02", "1358b7810830005dc0570000000000c80102080200"),
    ("P03", "145dc05700000058b7810830000000c80102080200"),
    ("P04", "105dc05700000058b781083000000100000000000048454c4c4f"),
    ("P05", "1158b7810830005dc0570000000002000100000000"),
    ("P06", "105dc05700000058b78108300000010100000000003733"),
    ("P07", "1158b7810830005dc0570000000002000200000000"),
    ("P08", "175dc05700000058b78108300000"),
    ("P09", "155dc05700000058b78108300000"),
    ("P10", "1658b78108300058b7810830000006564b33414243045146323201"),
    ("P11", "165dc0570000005dc0570000000004573141570001"),
    ("P12", "125dc05700000058b7810830000500c80102080200"),
    ("P13", "0200fc0000001b"),
    ("P14", "0200010000000005"),
    ("P15", "010700000000004849"),
    ("P16", "03000900000005"),
    ("P17", "04000000000000"),
    ("P18", "050000000000000101"),
    ("P19", "050001000000000201"),
    ("P20", "05000100000000"),
    ("P21", "050101000000000301"),
    ("P22", "05000200000000"),
    ("P23", "00c80102080200"),
    ("P24", "00c80102080000"),
    ("P25", "00c80304080305"),
    ("P26", "00780102040203"),
    ("P27", "00780102040203"),
    ("P28", "1001020001a0"),
    (
        "P29",
        "fe2b000000a002ff10ff5dffc0bffc030200e147fdbffc0ff846bff00ff00ff01f0600a00ff00ff00ff00a555540",
    ),
    ("N01", "1b5dc05700000058b781083000000100000000000048454c4c4f"),
    ("N02", "f05dc05700000058b781083000000100000000000048454c4c4f"),
    ("N03", "010000000000004142434445464748494a"),
    ("N04", "175dc05700000058b781083000"),
    ("N05", "17fc000000000058b78108300000"),
    ("N06", "06000000000000"),
    ("N07", "010000000000"),
    ("N08", "175dc05700000058b781083000f3"),
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

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn bytes_of(id: &str) -> Vec<u8> {
    unhex(vector(id))
}

fn call(s: &str) -> Callsign {
    Callsign::new(s).expect("valid callsign")
}

// ── Test-only decoder: SPEC.md §17/§18/§20 offsets, no production parser ──

/// §17.2: eight 6-bit codes packed MSB-first into 6 bytes; 0 pads, A-Z are
/// 1-26, 0-9 are 27-36, '/' is 37, '-' is 38. Decoding stops at the first 0.
fn unpack_callsign(field: &[u8]) -> String {
    assert_eq!(field.len(), 6);
    let bits = field.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    let mut text = String::new();
    for i in 0..8 {
        let code = ((bits >> (42 - 6 * i)) & 0x3F) as u8;
        match code {
            0 => break,
            1..=26 => text.push((b'A' + code - 1) as char),
            27..=36 => text.push((b'0' + code - 27) as char),
            37 => text.push('/'),
            38 => text.push('-'),
            _ => panic!("code {code} is outside the §17.2 charset"),
        }
    }
    text
}

/// §17.1 fields of a MAC PDU.
struct MacFields<'a> {
    version: u8,
    frame_type: u8,
    dest: String,
    src: String,
    ssid: u8,
    payload: &'a [u8],
}

fn split_mac(b: &[u8]) -> MacFields<'_> {
    assert!(b.len() >= 14, "a MAC PDU is at least 14 bytes");
    MacFields {
        version: b[0] >> 4,
        frame_type: b[0] & 0x0F,
        dest: unpack_callsign(&b[1..7]),
        src: unpack_callsign(&b[7..13]),
        ssid: b[13] & 0x0F,
        payload: &b[14..],
    }
}

/// §18.1 fields of a transport PDU.
struct TransportFields<'a> {
    session_id: u8,
    transport_type: u8,
    seq: u8,
    ack: u8,
    bitmap: u32,
    payload: &'a [u8],
}

fn split_transport(b: &[u8]) -> TransportFields<'_> {
    assert!(b.len() >= 7, "a transport PDU is at least 7 bytes");
    TransportFields {
        session_id: b[0] >> 4,
        transport_type: b[0] & 0x0F,
        seq: b[1],
        ack: b[2],
        bitmap: u32::from_be_bytes([b[3], b[4], b[5], b[6]]),
        payload: &b[7..],
    }
}

/// §20.1 capability fields: (max_payload, fec_num, fec_den, window,
/// compression, multi_frame_ack, reserved flag bits, mcs_index).
fn split_caps(b: &[u8]) -> (u16, u8, u8, u8, bool, bool, u8, u8) {
    assert_eq!(b.len(), 7, "a capabilities blob is 7 bytes");
    (
        u16::from_be_bytes([b[0], b[1]]),
        b[2],
        b[3],
        b[4],
        b[5] & 0x01 != 0,
        b[5] & 0x02 != 0,
        b[5] & 0xFC,
        b[6],
    )
}

/// Assert a MAC header by hand: (version 1, type, dest, src, ssid).
fn assert_mac(id: &str, frame_type: u8, dest: &str, src: &str, ssid: u8, len: usize) {
    let b = bytes_of(id);
    assert_eq!(b.len(), len, "{id} length");
    let m = split_mac(&b);
    assert_eq!(
        (
            m.version,
            m.frame_type,
            m.dest.as_str(),
            m.src.as_str(),
            m.ssid
        ),
        (1, frame_type, dest, src, ssid),
        "{id} MAC header"
    );
}

// ── Builders restating coppad's (see module doc) ──

fn session_data(from: &Callsign, to: &Callsign, seq: u8, body: &[u8]) -> Vec<u8> {
    let segment = TransportPdu::new_reliable(0, seq, 0, body.to_vec());
    MacPdu::new_data(to.clone(), from.clone(), 0, segment.to_bytes()).to_bytes()
}

fn session_ack(from: &Callsign, to: &Callsign, rx: &ArqRx) -> Vec<u8> {
    let (ack_num, bitmap) = rx.ack_info();
    let ack = TransportPdu::new_ack(0, ack_num, bitmap);
    MacPdu::new(
        MacFrameType::Ack,
        to.clone(),
        from.clone(),
        0,
        ack.to_bytes(),
    )
    .to_bytes()
}

fn coppad_caps() -> LinkCapabilities {
    LinkCapabilities {
        multi_frame_ack: true,
        ..LinkCapabilities::default()
    }
}

// ── Binding to SPEC.md §21 ──

fn spec_section_21() -> &'static str {
    // No trailing `\n` in the pattern, so a CRLF checkout still matches.
    let start = SPEC
        .find("\n## 21. Golden protocol vectors")
        .expect("SPEC.md has a `## 21. Golden protocol vectors` heading");
    let body = &SPEC[start + 1..];
    match body[3..].find("\n## ") {
        Some(end) => &body[..end + 3],
        None => body,
    }
}

/// Every vector has exactly one §21 table row starting `| <ID> |`, and that
/// row carries the frozen hex in backticks.
#[test]
fn spec_21_rows_match_frozen_vectors() {
    let section = spec_section_21();
    for (id, expected) in VECTORS {
        let prefix = format!("| {id} |");
        let rows: Vec<&str> = section
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(&prefix))
            .collect();
        assert_eq!(rows.len(), 1, "SPEC.md §21 must have one row for {id}");
        assert!(
            rows[0].contains(&format!("`{expected}`")),
            "SPEC.md §21 row {id} does not carry `{expected}`: {}",
            rows[0]
        );
    }
    // The other direction: every §21 row ID has a frozen vector here, so a
    // row added or renamed in SPEC.md without this file fails too.
    for line in section.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("| ") else {
            continue;
        };
        let id = rest.split(" |").next().unwrap_or("");
        let is_row_id = id.len() == 3
            && (id.starts_with('P') || id.starts_with('N'))
            && id[1..].bytes().all(|b| b.is_ascii_digit());
        if is_row_id {
            assert!(
                VECTORS.iter().any(|(v, _)| *v == id),
                "SPEC.md §21 row {id} has no frozen vector in this test"
            );
        }
    }
}

// ── P01-P12: the conversation ──

/// P01-P03: the handshake, driven through `Session`, reaches Established at
/// both ends and emits the frozen bytes.
#[test]
fn handshake_emits_p01_to_p03_and_establishes() {
    let a = call("VK3ABC");
    let b = call("W1AW");
    let now = Instant::now();
    let mut ini = Session::new(0, a.clone(), b.clone(), 0, coppad_caps(), now);
    let mut rsp = Session::new(0, b, a, 0, coppad_caps(), now);

    let req = ini.initiate(now).unwrap();
    assert_eq!(hex(&req.to_bytes()), vector("P01"));
    let ack = rsp.handle_connect_req(&req.payload, now).unwrap();
    assert_eq!(hex(&ack.to_bytes()), vector("P02"));
    let cfm = ini.handle_connect_ack(&ack.payload, now).unwrap();
    assert_eq!(hex(&cfm.to_bytes()), vector("P03"));
    rsp.handle_connect_cfm(&cfm.payload, now).unwrap();

    assert_eq!(ini.state, SessionState::Established);
    assert_eq!(rsp.state, SessionState::Established);
    assert_eq!(ini.negotiated.as_ref().unwrap().to_bytes(), bytes_of("P23"));

    // Hand decode: direction, type and the 7-byte caps payload.
    assert_mac("P01", 2, "W1AW", "VK3ABC", 0, 21);
    assert_mac("P02", 3, "VK3ABC", "W1AW", 0, 21);
    assert_mac("P03", 4, "W1AW", "VK3ABC", 0, 21);
    for id in ["P01", "P02", "P03"] {
        let b = bytes_of(id);
        assert_eq!(
            split_caps(split_mac(&b).payload),
            (200, 1, 2, 8, false, true, 0, 0),
            "{id} capabilities"
        );
    }
}

/// P12: the same ConnectReq from a session with SSID 5; only byte 13 moves.
#[test]
fn connect_req_with_ssid_5_is_p12() {
    let now = Instant::now();
    let mut ini = Session::new(0, call("VK3ABC"), call("W1AW"), 5, coppad_caps(), now);
    assert_eq!(hex(&ini.initiate(now).unwrap().to_bytes()), vector("P12"));
    assert_mac("P12", 2, "W1AW", "VK3ABC", 5, 21);
}

/// P04-P07: two Data frames, a retransmission and the two window-1 ACKs,
/// walked through a real `ArqRx` (duplicate re-ACKed, nothing redelivered).
#[test]
fn data_ack_exchange_is_p04_to_p07() {
    let a = call("VK3ABC");
    let b = call("W1AW");
    let mut rx = ArqRx::new(1);

    let data0 = session_data(&a, &b, 0, b"HELLO");
    assert_eq!(hex(&data0), vector("P04"));
    assert_eq!(
        rx.receive(0, b"HELLO".to_vec()),
        vec![(0, b"HELLO".to_vec())]
    );
    assert_eq!(rx.ack_info(), (1, 0));
    assert_eq!(hex(&session_ack(&b, &a, &rx)), vector("P05"));

    // A duplicate of seq 0 delivers nothing and is ACKed with the same bytes.
    assert!(rx.receive(0, b"HELLO".to_vec()).is_empty());
    assert_eq!(hex(&session_ack(&b, &a, &rx)), vector("P05"));

    let data1 = session_data(&a, &b, 1, b"73");
    assert_eq!(hex(&data1), vector("P06"));
    // A retransmission is rebuilt byte-identical (§19.4).
    assert_eq!(session_data(&a, &b, 1, b"73"), data1);
    // Out of window (seq 2 while 1 is expected, window 1) is dropped and the
    // ACK does not move.
    assert!(rx.receive(2, b"x".to_vec()).is_empty());
    assert_eq!(rx.ack_info(), (1, 0));
    assert_eq!(rx.receive(1, b"73".to_vec()), vec![(1, b"73".to_vec())]);
    assert_eq!(hex(&session_ack(&b, &a, &rx)), vector("P07"));

    // Hand decode.
    for (id, seq, body) in [("P04", 0u8, &b"HELLO"[..]), ("P06", 1, b"73")] {
        assert_mac(id, 0, "W1AW", "VK3ABC", 0, 21 + body.len());
        let b = bytes_of(id);
        let t = split_transport(split_mac(&b).payload);
        assert_eq!(
            (
                t.session_id,
                t.transport_type,
                t.seq,
                t.ack,
                t.bitmap,
                t.payload
            ),
            (0, 1, seq, 0, 0, body),
            "{id} transport"
        );
    }
    for (id, ack) in [("P05", 1u8), ("P07", 2)] {
        assert_mac(id, 1, "VK3ABC", "W1AW", 0, 21);
        let b = bytes_of(id);
        let t = split_transport(split_mac(&b).payload);
        assert_eq!(
            (
                t.session_id,
                t.transport_type,
                t.seq,
                t.ack,
                t.bitmap,
                t.payload
            ),
            (0, 2, 0, ack, 0, &[][..]),
            "{id} transport"
        );
    }
}

/// P08/P09: Keepalive and Disconnect are bare 14-byte MAC headers.
#[test]
fn keepalive_and_disconnect_are_p08_p09() {
    let now = Instant::now();
    let a = call("VK3ABC");
    let b = call("W1AW");
    let mut ini = Session::new(0, a.clone(), b.clone(), 0, coppad_caps(), now);
    let mut rsp = Session::new(0, b, a, 0, coppad_caps(), now);
    let req = ini.initiate(now).unwrap();
    let ack = rsp.handle_connect_req(&req.payload, now).unwrap();
    ini.handle_connect_ack(&ack.payload, now).unwrap();

    assert_eq!(hex(&ini.keepalive(now).unwrap().to_bytes()), vector("P08"));
    assert_eq!(hex(&ini.disconnect(now).unwrap().to_bytes()), vector("P09"));
    assert_mac("P08", 7, "W1AW", "VK3ABC", 0, 14);
    assert_mac("P09", 5, "W1AW", "VK3ABC", 0, 14);
}

/// P10/P11: station-ID beacons, dest = src = own callsign, SSID 0, with and
/// without a grid; payload `[len][callsign][len][grid][level]`.
#[test]
fn beacons_are_p10_p11() {
    for (id, cs, grid) in [("P10", "VK3ABC", Some("QF22")), ("P11", "W1AW", None)] {
        let sid = StationIdPayload {
            callsign: cs.to_string(),
            grid: grid.map(str::to_string),
            level: 1,
        };
        let own = call(cs);
        let pdu = MacPdu::new(
            MacFrameType::Beacon,
            own.clone(),
            own,
            0,
            sid.to_bytes().unwrap(),
        );
        assert_eq!(hex(&pdu.to_bytes()), vector(id));

        let b = bytes_of(id);
        let m = split_mac(&b);
        assert_eq!((m.version, m.frame_type, m.ssid), (1, 6, 0));
        assert_eq!((m.dest.as_str(), m.src.as_str()), (cs, cs));
        let p = m.payload;
        let cl = p[0] as usize;
        assert_eq!(&p[1..1 + cl], cs.as_bytes());
        let gl = p[1 + cl] as usize;
        let parsed_grid = (gl > 0).then(|| std::str::from_utf8(&p[2 + cl..2 + cl + gl]).unwrap());
        assert_eq!(parsed_grid, grid);
        assert_eq!(p[2 + cl + gl], 1, "level");
        assert_eq!(p.len(), 3 + cl + gl);
        assert_eq!(StationIdPayload::from_bytes(p).unwrap(), sid);
    }
}

// ── P13-P17: transport-only rows ──

/// P13: window-8 SACK across the sequence wrap, from a real `ArqRx`.
#[test]
fn sack_across_wrap_is_p13() {
    let mut rx = ArqRx::new(8);
    for seq in 0..250u8 {
        rx.receive(seq, vec![seq]);
    }
    for seq in [250u8, 251, 253, 254, 0, 1] {
        rx.receive(seq, vec![seq]);
    }
    assert_eq!(rx.ack_info(), (252, 0x0000_001b));
    let (ack_num, bitmap) = rx.ack_info();
    let pdu = TransportPdu::new_ack(0, ack_num, bitmap);
    assert_eq!(hex(&pdu.to_bytes()), vector("P13"));

    let b = bytes_of("P13");
    let t = split_transport(&b);
    assert_eq!((t.session_id, t.transport_type, t.seq), (0, 2, 0));
    assert_eq!((t.ack, t.bitmap, t.payload.len()), (252, 0x1b, 0));
    // §19.2 by hand: diff = seq - ack (mod 256); 0 → no, >128 → yes,
    // 1..=31 → bit diff-1, else no.
    let by_hand = |seq: u8| {
        let diff = seq.wrapping_sub(t.ack);
        match diff {
            0 => false,
            129..=255 => true,
            1..=31 => (t.bitmap >> (diff - 1)) & 1 == 1,
            _ => false,
        }
    };
    let parsed = TransportPdu::from_bytes(&b).unwrap();
    for (seq, acked) in [
        (250u8, true),
        (251, true),
        (252, false),
        (253, true),
        (254, true),
        (255, false),
        (0, true),
        (1, true),
    ] {
        assert_eq!(parsed.is_acked(seq), acked, "is_acked({seq})");
        assert_eq!(by_hand(seq), acked, "hand rule for {seq}");
    }
    // The 128 boundary: diff 128 is not acked, diff 129 is.
    assert!(!parsed.is_acked(252u8.wrapping_add(128)));
    assert!(parsed.is_acked(252u8.wrapping_add(129)));
    // Bit 31 is never read: diff 32 is false even with every bit set.
    let all = TransportPdu::new_ack(0, 10, u32::MAX);
    assert!(all.is_acked(10 + 31));
    assert!(!all.is_acked(10 + 32));
}

/// P14-P17: ACK with suggested level, standalone Reliable, Nak, Reset.
#[test]
fn transport_rows_p14_to_p17() {
    let with_rate = TransportPdu::new_ack_with_rate(0, 1, 0, 5);
    assert_eq!(hex(&with_rate.to_bytes()), vector("P14"));
    let parsed = TransportPdu::from_bytes(&bytes_of("P14")).unwrap();
    assert_eq!(parsed.suggested_rate(), Some(5));

    let reliable = TransportPdu::new_reliable(0, 7, 0, b"HI".to_vec());
    assert_eq!(hex(&reliable.to_bytes()), vector("P15"));

    let nak = TransportPdu::new_nak(0, 9, 5);
    assert_eq!(hex(&nak.to_bytes()), vector("P16"));

    let reset = TransportPdu::new_reset(0);
    assert_eq!(hex(&reset.to_bytes()), vector("P17"));

    for (id, ty, seq, ack, bitmap, payload) in [
        ("P14", 2u8, 0u8, 1u8, 0u32, &[5u8][..]),
        ("P15", 1, 7, 0, 0, b"HI"),
        ("P16", 3, 0, 9, 5, &[]),
        ("P17", 4, 0, 0, 0, &[]),
    ] {
        let b = bytes_of(id);
        let t = split_transport(&b);
        assert_eq!(
            (
                t.session_id,
                t.transport_type,
                t.seq,
                t.ack,
                t.bitmap,
                t.payload
            ),
            (0, ty, seq, ack, bitmap, payload),
            "{id}"
        );
    }
}

// ── P18-P22: CP-switch negotiation ──

/// P18-P22: the five CpControl PDUs of one ShortCp negotiation, built with
/// the daemon's constructors over real window-2 `ArqTx`/`ArqRx` pairs and
/// the `CpNegotiator` payload helpers (P = proposer, R = responder).
#[test]
fn cp_negotiation_is_p18_to_p22() {
    let now = Instant::now();
    let cfg = || ArqConfig::new(2, 5, Duration::from_secs(5)).unwrap();
    let (mut p_tx, mut p_rx) = (ArqTx::new(cfg()).unwrap(), ArqRx::new(2));
    let (mut r_tx, mut r_rx) = (ArqTx::new(cfg()).unwrap(), ArqRx::new(2));

    // 1. P → R: Propose ShortCp.
    let propose = CpNegotiator::propose_payload(CpMode::ShortCp);
    let seq = p_tx.send(propose.clone(), now).unwrap();
    let (an, bm) = p_rx.ack_info();
    let pdu1 = TransportPdu::new_cp_control_content(0, seq, an, bm, propose);
    assert_eq!(hex(&pdu1.to_bytes()), vector("P18"));

    // 2. R → P: Confirm, piggybacking R's ack of P's seq 0.
    r_rx.receive(pdu1.seq_num, pdu1.payload.clone());
    let Some(ContentAction::SendConfirm(confirm)) =
        CpNegotiator::on_content_received(&pdu1.payload)
    else {
        panic!("Propose must yield a Confirm");
    };
    let seq = r_tx.send(confirm.clone(), now).unwrap();
    let (an, bm) = r_rx.ack_info();
    let pdu2 = TransportPdu::new_cp_control_content(pdu1.session_id, seq, an, bm, confirm);
    assert_eq!(hex(&pdu2.to_bytes()), vector("P19"));
    assert_eq!(
        p_tx.process_ack(pdu2.ack_num, pdu2.ack_bitmap, now),
        vec![0]
    );

    // 3. P → R: bare ack of the Confirm (old profile).
    p_rx.receive(pdu2.seq_num, pdu2.payload.clone());
    assert_eq!(
        CpNegotiator::on_content_received(&pdu2.payload),
        Some(ContentAction::ApplyAsConfirmer(CpMode::ShortCp))
    );
    let (an, bm) = p_rx.ack_info();
    let pdu3 = TransportPdu::new_cp_control_ack(pdu2.session_id, an, bm);
    assert_eq!(hex(&pdu3.to_bytes()), vector("P20"));
    assert_eq!(
        r_tx.process_ack(pdu3.ack_num, pdu3.ack_bitmap, now),
        vec![0]
    );

    // 4. R → P: Switched (new profile).
    let switched = CpNegotiator::switched_payload(CpMode::ShortCp);
    let seq = r_tx.send(switched.clone(), now).unwrap();
    let (an, bm) = r_rx.ack_info();
    let pdu4 = TransportPdu::new_cp_control_content(0, seq, an, bm, switched);
    assert_eq!(hex(&pdu4.to_bytes()), vector("P21"));

    // 5. P → R: bare ack of Switched (new profile).
    p_rx.receive(pdu4.seq_num, pdu4.payload.clone());
    assert_eq!(
        CpNegotiator::on_content_received(&pdu4.payload),
        Some(ContentAction::PeerSwitched(CpMode::ShortCp))
    );
    let (an, bm) = p_rx.ack_info();
    let pdu5 = TransportPdu::new_cp_control_ack(0, an, bm);
    assert_eq!(hex(&pdu5.to_bytes()), vector("P22"));
    assert_eq!(
        r_tx.process_ack(pdu5.ack_num, pdu5.ack_bitmap, now),
        vec![1]
    );

    // Hand decode: (seq, ack, [kind, mode]) per row, all type 5, bitmap 0.
    for (id, seq, ack, payload) in [
        ("P18", 0u8, 0u8, &[0x01u8, 0x01][..]),
        ("P19", 0, 1, &[0x02, 0x01]),
        ("P20", 0, 1, &[]),
        ("P21", 1, 1, &[0x03, 0x01]),
        ("P22", 0, 2, &[]),
    ] {
        let b = bytes_of(id);
        let t = split_transport(&b);
        assert_eq!(
            (
                t.session_id,
                t.transport_type,
                t.seq,
                t.ack,
                t.bitmap,
                t.payload
            ),
            (0, 5, seq, ack, 0, payload),
            "{id}"
        );
    }
    // A mode byte other than 0x01 is LongCp; any non-2-byte payload is ignored.
    assert_eq!(
        CpNegotiator::on_content_received(&[0x01, 0x7f]),
        Some(ContentAction::SendConfirm(vec![0x02, 0x00]))
    );
    assert_eq!(CpNegotiator::on_content_received(&[0x01]), None);
    assert_eq!(CpNegotiator::on_content_received(&[0x04, 0x01]), None);
}

// ── P23-P27: capabilities and negotiation ──

#[test]
fn capabilities_are_p23_to_p27() {
    assert_eq!(hex(&coppad_caps().to_bytes()), vector("P23"));
    assert_eq!(hex(&LinkCapabilities::default().to_bytes()), vector("P24"));
    assert_eq!(
        split_caps(&bytes_of("P24")),
        (200, 1, 2, 8, false, false, 0, 0)
    );

    let a = LinkCapabilities {
        max_payload_size: 200,
        fec_rate_num: 3,
        fec_rate_den: 4,
        arq_window: 8,
        compression: true,
        multi_frame_ack: true,
        outer_code_rx: false,
        outer_code_req: false,
        app_messages: false,
        mcs_index: 5,
    };
    let b = LinkCapabilities {
        max_payload_size: 120,
        fec_rate_num: 1,
        fec_rate_den: 2,
        arq_window: 4,
        compression: false,
        multi_frame_ack: true,
        outer_code_rx: false,
        outer_code_req: false,
        app_messages: false,
        mcs_index: 3,
    };
    assert_eq!(hex(&a.to_bytes()), vector("P25"));
    assert_eq!(hex(&b.to_bytes()), vector("P26"));
    assert_eq!(hex(&a.negotiate(&b).to_bytes()), vector("P27"));
    assert_eq!(hex(&b.negotiate(&a).to_bytes()), vector("P27"));
    assert_eq!(
        split_caps(&bytes_of("P25")),
        (200, 3, 4, 8, true, true, 0, 5)
    );
    assert_eq!(
        split_caps(&bytes_of("P27")),
        (120, 1, 2, 4, false, true, 0, 3)
    );

    // Reserved flag bits (5–7; bits 2–3 are the outer-code capability of
    // §22.1 and bit 4 is COP-121's `app_messages`, §23) and trailing bytes
    // are ignored on read; fewer than 7 bytes is an error.
    let mut lenient = bytes_of("P23");
    lenient[5] |= 0xE0;
    lenient.push(0xAA);
    assert_eq!(
        LinkCapabilities::from_bytes(&lenient).unwrap(),
        coppad_caps()
    );
    let mut chat = bytes_of("P23");
    chat[5] |= 0x10;
    assert_eq!(
        LinkCapabilities::from_bytes(&chat).unwrap(),
        LinkCapabilities {
            app_messages: true,
            ..coppad_caps()
        }
    );
    assert!(matches!(
        LinkCapabilities::from_bytes(&bytes_of("P23")[..6]),
        Err(ProtocolError::CapabilitiesTooShort { actual: 6 })
    ));
}

// ── P28/P29: the layers below MAC ──

/// P28: the PHY header (SPEC.md §4.2) coppad stamps over P04 at level 2 on
/// `hf_standard`: wire version 1, phy_mode 0, frame_type Data, fec_type 0,
/// seq 0, payload_len 26, one codeword.
#[test]
fn phy_header_over_p04_is_p28() {
    let header = CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: CoppaProfile::hf_standard().bandwidth_id,
        fec_type: 0,
        speed_level: 2,
        seq_num: 0,
        payload_len: bytes_of("P04").len() as u16,
        codewords: 1,
    };
    assert_eq!(hex(&header.to_bytes()), vector("P28"));
    let h = bytes_of("P28");
    assert_eq!((h[0] >> 4, h[0] & 0x0F), (1, 0), "version, phy_mode");
    assert_eq!((h[1] >> 4, h[1] & 0x0F), (0, 1), "frame_type, bandwidth");
    assert_eq!((h[2] >> 4, h[2] & 0x0F), (0, 2), "fec_type, speed_level");
    assert_eq!(h[3], 0, "seq_num");
    assert_eq!(
        (u16::from(h[4]) << 4) | u16::from(h[5] >> 4),
        26,
        "payload_len"
    );
    assert_eq!((h[5] & 0x0F) + 1, 1, "codewords");
}

/// P29: the §15 envelope wraps the whole MAC PDU (headers included). Pins
/// the current `lz4_flex` encoding, not the only valid LZ4 representation.
#[test]
fn compression_envelope_over_p04_is_p29() {
    let codec = HuffmanCodec::new();
    let mut envelope = vec![0xFE];
    envelope.extend_from_slice(&lz4_compress(&codec.encode(&bytes_of("P04"))));
    assert_eq!(hex(&envelope), vector("P29"));
    let inner = lz4_decompress(&envelope[1..]).unwrap();
    assert_eq!(codec.decode(&inner), bytes_of("P04"));
}

// ── N01-N08: parser results the reference receiver gives ──

#[test]
fn negative_and_tolerance_rows_n01_to_n08() {
    // N01: MAC type nibble 11 is unknown; coppad then tries transport/raw.
    assert!(matches!(
        MacPdu::from_bytes(&bytes_of("N01")),
        Err(ProtocolError::UnknownMacFrameType { value: 11 })
    ));

    // N02: the MAC version nibble is not checked.
    let n02 = MacPdu::from_bytes(&bytes_of("N02")).unwrap();
    assert_eq!((n02.version, n02.frame_type), (15, MacFrameType::Data));

    // N03: a bare transport Reliable that is also a valid MAC Ack (§16.3).
    let n03 = bytes_of("N03");
    let t = TransportPdu::from_bytes(&n03).unwrap();
    assert_eq!(
        (t.transport_type, t.seq_num, t.payload.as_slice()),
        (TransportType::Reliable, 0, &b"ABCDEFGHIJ"[..])
    );
    let m = MacPdu::from_bytes(&n03).unwrap();
    assert_eq!(
        (
            m.version,
            m.frame_type,
            m.dest.as_str(),
            m.src.as_str(),
            m.ssid
        ),
        (0, MacFrameType::Ack, "", "PTICQDUF", 7)
    );
    let hand = split_mac(&n03);
    assert_eq!((hand.dest.as_str(), hand.src.as_str()), ("", "PTICQDUF"));

    // N04: 13 bytes is shorter than the MAC header.
    assert!(matches!(
        MacPdu::from_bytes(&bytes_of("N04")),
        Err(ProtocolError::MacPduTooShort {
            actual: 13,
            min: 14
        })
    ));

    // N05: dest's first 6-bit code is 63, outside the charset.
    assert!(matches!(
        MacPdu::from_bytes(&bytes_of("N05")),
        Err(ProtocolError::InvalidCallsignCode { code: 63 })
    ));

    // N06: transport type nibble 6 is unknown.
    assert!(matches!(
        TransportPdu::from_bytes(&bytes_of("N06")),
        Err(ProtocolError::UnknownTransportType { value: 6 })
    ));

    // N07: 6 bytes is shorter than the transport header.
    assert!(matches!(
        TransportPdu::from_bytes(&bytes_of("N07")),
        Err(ProtocolError::TransportPduTooShort { actual: 6, min: 7 })
    ));

    // N08: only the SSID byte's low nibble is read.
    let n08 = MacPdu::from_bytes(&bytes_of("N08")).unwrap();
    assert_eq!((n08.frame_type, n08.ssid), (MacFrameType::Keepalive, 3));

    // Callsign text, not SSID: "VK3ABC-1" is an 8-character callsign.
    assert_eq!(hex(&call("VK3ABC-1").encode()), "58b78108399c");
    assert_eq!(unpack_callsign(&unhex("58b78108399c")), "VK3ABC-1");
    assert_eq!(hex(&Callsign::broadcast().encode()), "000000000000");
}

// ── §16.3 and §19.5: the two documented sessionless hazards ──

/// SPEC.md §16.3: a bare transport `Reliable` PDU's MAC `dest` field is the
/// bytes `seq, 0, 0, 0, 0, 0`, which decode as a callsign for 118 of the 256
/// sequence numbers (0-3, and 4-155 except those with both low bits set).
#[test]
fn bare_reliable_seq_gives_a_valid_mac_dest_for_118_values() {
    let valid: Vec<u8> = (0..=255u8)
        .filter(|&seq| Callsign::decode(&[seq, 0, 0, 0, 0, 0]).is_ok())
        .collect();
    assert_eq!(valid.len(), 118);
    assert!(valid
        .iter()
        .all(|&seq| seq <= 3 || (seq <= 155 && seq & 3 != 3)));
}

/// SPEC.md §19.2/§19.5 and docs/LIMITATIONS.md: `ArqTx::process_ack`
/// advances `send_base` across a distance of 1-128, so the all-zero ACK
/// fields of a sessionless data PDU acknowledge in-flight segments that
/// were never received once `send_base` reaches 128.
#[test]
fn sessionless_zero_ack_moves_send_base_from_128() {
    let now = Instant::now();
    // `base` segments sent and each acknowledged, then one more in flight.
    let tx_with_one_in_flight = |base: u8| {
        let config = ArqConfig::new(8, 5, Duration::from_secs(5)).unwrap();
        let mut tx = ArqTx::new(config).unwrap();
        for _ in 0..base {
            let seq = tx.send(vec![0], now).unwrap();
            assert_eq!(tx.process_ack(seq.wrapping_add(1), 0, now), vec![seq]);
        }
        assert_eq!(tx.send(vec![0], now).unwrap(), base);
        assert_eq!((tx.send_base(), tx.in_flight()), (base, 1));
        tx
    };

    // send_base 127: the distance to ack 0 is 129, so nothing moves.
    let mut tx = tx_with_one_in_flight(127);
    assert!(tx.process_ack(0, 0, now).is_empty());
    assert_eq!((tx.send_base(), tx.in_flight()), (127, 1));

    // send_base 128 (d = 128 for `is_acked`, which says "not acked"): the
    // sender acknowledges 128 and wraps send_base to 0.
    let mut tx = tx_with_one_in_flight(128);
    assert!(!TransportPdu::new_ack(0, 0, 0).is_acked(128));
    assert_eq!(tx.process_ack(0, 0, now), vec![128]);
    assert_eq!((tx.send_base(), tx.in_flight()), (0, 129));

    // The case docs/LIMITATIONS.md quotes: 130 acknowledged sends.
    let mut tx = tx_with_one_in_flight(130);
    assert_eq!(tx.process_ack(0, 0, now), vec![130]);
    assert_eq!((tx.send_base(), tx.in_flight()), (0, 131));
}
