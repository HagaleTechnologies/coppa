//! End-to-end AFSK 1200 / AX.25 loopback integration tests.
//!
//! These tests prove the full pipeline:
//!   AX.25 encode → AFSK modulate → (noise) → AFSK demodulate → AX.25 decode

use coppa_codec::afsk::{modulate, Demodulator};
use coppa_protocol::ax25::{Ax25Address, Ax25Frame};

// ── Helper ────────────────────────────────────────────────────────────────────

fn make_ax25_frame(src: &str, dest: &str, info: &str) -> Vec<u8> {
    let frame = Ax25Frame {
        dest: Ax25Address {
            callsign: dest.to_string(),
            ssid: 0,
        },
        src: Ax25Address {
            callsign: src.to_string(),
            ssid: 0,
        },
        digipeaters: vec![],
        info: info.as_bytes().to_vec(),
    };
    frame.to_bytes()
}

/// Run the modulate → demodulate pipeline and return raw frame bytes extracted.
fn loopback(frame_bytes: &[u8]) -> Vec<Vec<u8>> {
    let samples = modulate(frame_bytes);
    let mut demod = Demodulator::new();
    demod.process(&samples);
    demod.take_frames()
}

/// Same as `loopback` but with AWGN added after modulation.
fn loopback_noisy(frame_bytes: &[u8], snr_db: f32, seed: u64) -> Vec<Vec<u8>> {
    let samples = modulate(frame_bytes);
    let noisy = coppa_channel::awgn_seeded(&samples, snr_db, seed);
    let mut demod = Demodulator::new();
    demod.process(&noisy);
    demod.take_frames()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Basic clean loopback: encode a simple frame, modulate, demodulate, decode.
#[test]
fn test_afsk_ax25_loopback_clean() {
    let frame_bytes = make_ax25_frame("W1AW", "CQ", "Hello from Coppa!");

    let frames = loopback(&frame_bytes);
    assert_eq!(frames.len(), 1, "Expected 1 frame, got {}", frames.len());

    let decoded = Ax25Frame::from_bytes(&frames[0]).expect("AX.25 decode failed");
    assert_eq!(decoded.src.callsign, "W1AW");
    assert_eq!(decoded.dest.callsign, "CQ");
    assert_eq!(decoded.info, b"Hello from Coppa!");
    assert!(decoded.digipeaters.is_empty());
}

/// Loopback through AWGN channel at 15 dB SNR — should decode cleanly.
#[test]
fn test_afsk_ax25_loopback_noisy_15db() {
    let frame_bytes = make_ax25_frame("W1AW", "CQ", "Hello from Coppa!");

    let frames = loopback_noisy(&frame_bytes, 15.0, 123);
    assert_eq!(
        frames.len(),
        1,
        "Expected 1 frame at 15 dB SNR, got {}",
        frames.len()
    );

    let decoded = Ax25Frame::from_bytes(&frames[0]).expect("AX.25 decode failed at 15 dB");
    assert_eq!(decoded.src.callsign, "W1AW");
    assert_eq!(decoded.dest.callsign, "CQ");
    assert_eq!(decoded.info, b"Hello from Coppa!");
}

/// Loopback through AWGN channel at 10 dB SNR — AFSK 1200 should handle this.
#[test]
fn test_afsk_ax25_loopback_noisy_10db() {
    let frame_bytes = make_ax25_frame("W1AW", "CQ", "Hello from Coppa!");

    let frames = loopback_noisy(&frame_bytes, 10.0, 456);
    assert_eq!(
        frames.len(),
        1,
        "Expected 1 frame at 10 dB SNR, got {}",
        frames.len()
    );

    let decoded = Ax25Frame::from_bytes(&frames[0]).expect("AX.25 decode failed at 10 dB");
    assert_eq!(decoded.src.callsign, "W1AW");
    assert_eq!(decoded.dest.callsign, "CQ");
    assert_eq!(decoded.info, b"Hello from Coppa!");
}

/// Frame with digipeaters (APRS WIDE1-1, WIDE2-1 style).
#[test]
fn test_afsk_ax25_loopback_with_digipeaters() {
    let frame = Ax25Frame {
        dest: Ax25Address {
            callsign: "APRS".to_string(),
            ssid: 0,
        },
        src: Ax25Address {
            callsign: "W1AW".to_string(),
            ssid: 9,
        },
        digipeaters: vec![
            Ax25Address {
                callsign: "WIDE1".to_string(),
                ssid: 1,
            },
            Ax25Address {
                callsign: "WIDE2".to_string(),
                ssid: 1,
            },
        ],
        info: b"!4903.50N/07201.75W-Test APRS".to_vec(),
    };
    let frame_bytes = frame.to_bytes();

    let frames = loopback(&frame_bytes);
    assert_eq!(
        frames.len(),
        1,
        "Expected 1 frame with digipeaters, got {}",
        frames.len()
    );

    let decoded = Ax25Frame::from_bytes(&frames[0]).expect("AX.25 decode failed (digipeaters)");
    assert_eq!(decoded.src.callsign, "W1AW");
    assert_eq!(decoded.src.ssid, 9);
    assert_eq!(decoded.dest.callsign, "APRS");
    assert_eq!(decoded.digipeaters.len(), 2);
    assert_eq!(decoded.digipeaters[0].callsign, "WIDE1");
    assert_eq!(decoded.digipeaters[0].ssid, 1);
    assert_eq!(decoded.digipeaters[1].callsign, "WIDE2");
    assert_eq!(decoded.digipeaters[1].ssid, 1);
    assert_eq!(decoded.info, b"!4903.50N/07201.75W-Test APRS");
}

/// Two different frames with 100 ms silence between them — both must decode.
#[test]
fn test_afsk_ax25_multiple_frames_loopback() {
    let frame1_bytes = make_ax25_frame("W1AW", "CQ", "Frame one");
    let frame2_bytes = make_ax25_frame("KD9XYZ", "APRS", "Frame two");

    let samples1 = modulate(&frame1_bytes);
    let samples2 = modulate(&frame2_bytes);

    // 100 ms silence at 48 kHz = 4800 samples
    let silence = vec![0.0f32; 4800];

    let mut combined = samples1;
    combined.extend_from_slice(&silence);
    combined.extend_from_slice(&samples2);

    let mut demod = Demodulator::new();
    demod.process(&combined);
    let frames = demod.take_frames();

    assert_eq!(frames.len(), 2, "Expected 2 frames, got {}", frames.len());

    let decoded1 = Ax25Frame::from_bytes(&frames[0]).expect("AX.25 decode failed (frame 1)");
    assert_eq!(decoded1.src.callsign, "W1AW");
    assert_eq!(decoded1.dest.callsign, "CQ");
    assert_eq!(decoded1.info, b"Frame one");

    let decoded2 = Ax25Frame::from_bytes(&frames[1]).expect("AX.25 decode failed (frame 2)");
    assert_eq!(decoded2.src.callsign, "KD9XYZ");
    assert_eq!(decoded2.dest.callsign, "APRS");
    assert_eq!(decoded2.info, b"Frame two");
}
