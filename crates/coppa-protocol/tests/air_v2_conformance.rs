//! Air-protocol version 2 conformance for the low-rate floor framing
//! (COP-190). Every floor frame type round-trips, a version-mismatched
//! header is rejected, and the version-1 contract stays frozen.

use coppa_codec::floor::FloorFsk;
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
use coppa_protocol::modem::floor::{
    FloorError, FloorFrame, FLOOR_AIR_PROTOCOL_VERSION, FRAME_SAMPLES, MAX_PAYLOAD,
};
use coppa_protocol::session::LinkCapabilities;
use coppa_protocol::AIR_PROTOCOL_VERSION;

/// The CONNECT-family PDUs a floor frame carries on the session path, at
/// every legal payload length.
fn connect_pdus() -> Vec<Vec<u8>> {
    let mut pdus = Vec::new();
    for frame_type in [
        MacFrameType::ConnectReq,
        MacFrameType::ConnectAck,
        MacFrameType::ConnectCfm,
    ] {
        pdus.push(
            MacPdu::new(
                frame_type,
                Callsign::new("W1AW").unwrap(),
                Callsign::new("K1ABC").unwrap(),
                0,
                LinkCapabilities::default().to_bytes(),
            )
            .to_bytes(),
        );
    }
    for len in [0, 1, 13, MAX_PAYLOAD] {
        pdus.push((0..len as u8).collect());
    }
    pdus
}

#[test]
fn version_two_is_allocated_and_version_one_stays_frozen() {
    assert_eq!(FLOOR_AIR_PROTOCOL_VERSION, 2);
    assert_eq!(AIR_PROTOCOL_VERSION, 1);
    assert_eq!(coppa_protocol::mac::MAC_VERSION, 1);
}

#[test]
fn round_trips_every_floor_frame_type() {
    let floor = FloorFrame::new();
    for pdu in connect_pdus() {
        assert!(pdu.len() <= MAX_PAYLOAD);
        let audio = floor.encode(&pdu).unwrap();
        assert_eq!(audio.len(), FRAME_SAMPLES);
        assert!(audio.len() <= 8 * 48_000, "8.0 s airtime freeze");
        assert_eq!(floor.receive(&audio).unwrap(), pdu);
    }
}

#[test]
fn capacity_is_frozen_at_forty_bytes() {
    let floor = FloorFrame::new();
    assert_eq!(
        floor.encode(&[0u8; MAX_PAYLOAD + 1]).unwrap_err(),
        FloorError::Capacity
    );
    assert_eq!(
        floor.encode(&[0u8; MAX_PAYLOAD]).unwrap().len(),
        FRAME_SAMPLES
    );
}

/// Overwrite the four repeated headers' first byte with the tones of
/// `version`, the way a mismatched transmitter would stamp them.
fn forge_header_version(audio: &mut [f32], floor: &FloorFsk, version: u8) {
    let header = [version, 4, 0, 0];
    for rep in 0..4 {
        for (byte, value) in header.iter().enumerate() {
            for (nibble, tone) in [value >> 4, value & 15].into_iter().enumerate() {
                let start = (16 + rep * 8 + byte * 2 + nibble) * 960;
                audio[start..start + 960].copy_from_slice(floor.tone_waveform(tone.into()));
            }
        }
    }
}

#[test]
fn rejects_version_mismatched_headers() {
    let floor = FloorFrame::new();
    let fsk = FloorFsk::new();
    for version in [0u8, 1, 3, 15] {
        let mut audio = floor.encode(b"test").unwrap();
        forge_header_version(&mut audio, &fsk, version);
        assert_ne!(
            floor.receive(&audio).unwrap_err(),
            FloorError::Capacity,
            "version {version} must fail before capacity"
        );
        assert!(
            matches!(
                floor.receive(&audio),
                Err(FloorError::Header | FloorError::Acquisition)
            ),
            "version {version} must not deliver"
        );
    }
    // The unmodified version-2 header still delivers the same payload.
    let audio = floor.encode(b"test").unwrap();
    assert_eq!(floor.receive(&audio).unwrap(), b"test".to_vec());
}

#[test]
fn v1_ofdm_vectors_are_unaffected_by_the_floor_module() {
    // The floor shares no state with the OFDM transceiver; a v1 frame
    // round-trips through the untouched production path while the floor
    // module is linked in.
    use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
    use coppa_codec::ofdm::CoppaProfile;
    use coppa_protocol::modem::transceiver::CoppaTransceiver;
    let payload = b"v1 still works";
    let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), AIR_PROTOCOL_VERSION);
    let header = CoppaHeader {
        version: AIR_PROTOCOL_VERSION,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: transceiver.profile().bandwidth_id,
        fec_type: 0,
        speed_level: 1,
        seq_num: 0,
        payload_len: payload.len() as u16,
        codewords: 1,
    };
    let audio = transceiver.transmit(&header, payload).expect("v1 transmit");
    let (rx_header, rx_payload, _) = transceiver.receive(&audio).expect("v1 receive");
    assert_eq!(rx_header.speed_level, 1);
    assert_eq!(rx_payload, payload);
}
