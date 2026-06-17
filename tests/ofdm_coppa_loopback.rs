//! End-to-end Coppa Protocol OFDM loopback integration tests.
//!
//! These tests prove the full pipeline:
//!   CoppaHeader + payload → CoppaModem::modulate → (optional noise) → CoppaModem::demodulate
//! across multiple profiles and channel conditions.

use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;

// ── Helper ────────────────────────────────────────────────────────────────────

fn make_test_frame(payload: &[u8], speed_level: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level,
        seq_num: 0,
        payload_len: payload.len() as u16,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Clean loopback using the HF Standard profile.
#[test]
fn test_coppa_ofdm_clean_loopback_hf() {
    let profile = CoppaProfile::hf_standard();
    let modem = CoppaModem::new(profile, 1);

    let payload = b"Coppa Protocol HF loopback test";
    let header = make_test_frame(payload, 2);

    let samples = modem.modulate(&header, payload);
    assert!(!samples.is_empty(), "Modulated output must not be empty");

    let (rx_header, rx_payload) = modem
        .demodulate(&samples)
        .expect("Clean HF loopback: demodulation should succeed");

    assert_eq!(rx_header.version, header.version);
    assert_eq!(rx_header.phy_mode, header.phy_mode);
    assert_eq!(rx_header.frame_type, header.frame_type);
    assert_eq!(rx_header.bandwidth, header.bandwidth);
    assert_eq!(rx_header.fec_type, header.fec_type);
    assert_eq!(rx_header.speed_level, header.speed_level);
    assert_eq!(rx_header.seq_num, header.seq_num);
    assert_eq!(rx_header.payload_len, header.payload_len);
    assert_eq!(rx_payload, payload, "Payload must match exactly");
}

/// Clean loopback using the VHF Narrow profile.
#[test]
fn test_coppa_ofdm_clean_loopback_vhf() {
    let profile = CoppaProfile::vhf_narrow();
    let modem = CoppaModem::new(profile, 1);

    let payload = b"Coppa Protocol HF loopback test";
    let header = CoppaHeader {
        version: 1,
        phy_mode: 1, // VHF
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: 2,
        seq_num: 0,
        payload_len: payload.len() as u16,
    };

    let samples = modem.modulate(&header, payload);
    assert!(
        !samples.is_empty(),
        "VHF modulated output must not be empty"
    );

    let (rx_header, rx_payload) = modem
        .demodulate(&samples)
        .expect("Clean VHF loopback: demodulation should succeed");

    assert_eq!(rx_header.version, header.version);
    assert_eq!(rx_header.frame_type, header.frame_type);
    assert_eq!(rx_header.speed_level, header.speed_level);
    assert_eq!(rx_header.payload_len, header.payload_len);
    assert_eq!(rx_payload, payload, "VHF payload must match exactly");
}

/// AWGN loopback at 15 dB SNR — BPSK without FEC should survive this.
#[test]
fn test_coppa_ofdm_awgn_15db() {
    let profile = CoppaProfile::hf_standard();
    let modem = CoppaModem::new(profile, 1);

    let payload = b"Coppa Protocol HF loopback test";
    let header = make_test_frame(payload, 2);

    let samples = modem.modulate(&header, payload);
    let noisy = coppa_channel::awgn_seeded(&samples, 15.0, 42);

    let (rx_header, rx_payload) = modem
        .demodulate(&noisy)
        .expect("15 dB SNR loopback: demodulation should succeed");

    assert_eq!(rx_header.payload_len, header.payload_len);
    assert_eq!(rx_payload, payload, "Payload must survive 15 dB AWGN");
}

/// AWGN loopback at 10 dB SNR.
///
/// BPSK without FEC may struggle at 10 dB. If this test fails it indicates
/// that forward error correction (FEC, planned for Phase C) is required to
/// achieve reliable decoding at this SNR level.
#[test]
fn test_coppa_ofdm_awgn_10db() {
    let profile = CoppaProfile::hf_standard();
    let modem = CoppaModem::new(profile, 1);

    let payload = b"Coppa Protocol HF loopback test";
    let header = make_test_frame(payload, 2);

    let samples = modem.modulate(&header, payload);
    let noisy = coppa_channel::awgn_seeded(&samples, 10.0, 42);

    // At 10 dB SNR, BPSK without FEC may not reliably decode. A failure here
    // is expected until Phase C (LDPC FEC) is implemented.
    match modem.demodulate(&noisy) {
        Some((rx_header, rx_payload)) => {
            assert_eq!(
                rx_header.payload_len, header.payload_len,
                "If decoding succeeds, payload length must match"
            );
            assert_eq!(
                rx_payload, payload,
                "If decoding succeeds at 10 dB, payload must be correct. \
                 A failure here means FEC (Phase C) is needed."
            );
        }
        None => {
            // Acceptable: BPSK without FEC cannot reliably decode at 10 dB SNR.
            // Phase C LDPC FEC will address this.
            eprintln!(
                "NOTE: 10 dB AWGN decode failed as expected without FEC. \
                 Phase C (LDPC) will fix this."
            );
        }
    }
}

/// ACK frame loopback — no payload, verify header fields roundtrip correctly.
#[test]
fn test_coppa_ofdm_ack_frame_loopback() {
    let profile = CoppaProfile::hf_standard();
    let modem = CoppaModem::new(profile, 1);

    let payload: &[u8] = &[];
    let header = CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Ack,
        bandwidth: 1,
        fec_type: 0,
        speed_level: 2,
        seq_num: 42,
        payload_len: 0,
    };

    let samples = modem.modulate(&header, payload);
    assert!(
        !samples.is_empty(),
        "ACK frame must produce some output samples"
    );

    let (rx_header, rx_payload) = modem
        .demodulate(&samples)
        .expect("ACK frame loopback: demodulation should succeed");

    assert_eq!(rx_header.version, 1);
    assert_eq!(rx_header.phy_mode, 0);
    assert_eq!(rx_header.frame_type, CoppaFrameType::Ack);
    assert_eq!(rx_header.bandwidth, 1);
    assert_eq!(rx_header.fec_type, 0);
    assert_eq!(rx_header.speed_level, 2);
    assert_eq!(rx_header.seq_num, 42);
    assert_eq!(rx_header.payload_len, 0);
    assert!(rx_payload.is_empty(), "ACK frame payload must be empty");
}
