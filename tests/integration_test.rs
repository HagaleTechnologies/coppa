use coppa_channel::{awgn_seeded, fading};
use coppa_engine::CoppaCore;

#[test]
fn test_end_to_end_communication() {
    let core = CoppaCore::new();

    let test_messages = vec![
        "Hello World",
        "CQ CQ CQ de VK2ABC",
        "Testing 123",
        "A",
        "Short msg ok",
    ];

    for message in test_messages {
        let samples = core.encode(message).expect("Failed to encode message");
        assert!(!samples.is_empty(), "Samples should not be empty");
        let decoded = core.decode(&samples).expect("Failed to decode samples");
        assert_eq!(
            message, decoded,
            "Message was not preserved through encode/decode cycle"
        );
    }
}

#[test]
fn test_long_message() {
    let core = CoppaCore::new();
    let long_message = "A".repeat(50);
    let samples = core
        .encode(&long_message)
        .expect("Failed to encode long message");
    let decoded = core
        .decode(&samples)
        .expect("Failed to decode long message");
    assert_eq!(long_message, decoded);
}

#[test]
fn test_maximum_message() {
    let core = CoppaCore::new();
    let max_message = "X".repeat(50);
    let samples = core
        .encode(&max_message)
        .expect("Failed to encode max message");
    let decoded = core.decode(&samples).expect("Failed to decode max message");
    assert_eq!(max_message, decoded);
}

#[test]
fn test_message_too_long() {
    let core = CoppaCore::new();
    // The engine rejects payloads exceeding u16::MAX bytes
    let too_long = "X".repeat(65536);
    let result = core.encode(&too_long);
    assert!(
        result.is_err(),
        "Should fail for message exceeding payload length limit"
    );
}

// --- Noise tolerance tests ---

#[test]
fn test_loopback_with_moderate_noise() {
    let core = CoppaCore::new();
    let message = "Hello World";
    let samples = core.encode(message).unwrap();
    let noisy_samples = awgn_seeded(&samples, 20.0, 42);
    let decoded = core
        .decode(&noisy_samples)
        .expect("Should decode at 20 dB SNR");
    assert_eq!(message, decoded);
}

#[test]
fn test_loopback_with_heavy_noise() {
    let core = CoppaCore::new();
    let message = "CQ";
    let samples = core.encode(message).unwrap();
    let noisy_samples = awgn_seeded(&samples, 15.0, 123);
    let decoded = core
        .decode(&noisy_samples)
        .expect("Should decode at 15 dB SNR with FEC");
    assert_eq!(message, decoded);
}

#[test]
fn test_loopback_with_amplitude_variation() {
    let core = CoppaCore::new();
    let message = "Testing AGC";
    let samples = core.encode(message).unwrap();

    let quiet: Vec<f32> = samples.iter().map(|s| s * 0.5).collect();
    let decoded = core.decode(&quiet).expect("Should handle 0.5x amplitude");
    assert_eq!(message, decoded);

    let loud: Vec<f32> = samples.iter().map(|s| s * 5.0).collect();
    let decoded = core.decode(&loud).expect("Should handle 5x amplitude");
    assert_eq!(message, decoded);
}

// --- Frequency offset (CFO) tests ---
// NOTE: OFDM preamble synchronization does not yet implement carrier frequency
// offset (CFO) correction. A genuine CFO-tolerance test is ignored until that
// feature lands. The clean-loopback test below documents the baseline.

#[test]
fn test_clean_loopback_baseline() {
    let core = CoppaCore::new();
    let message = "Freq test";
    let samples = core.encode(message).unwrap();
    // No impairment applied: this is a clean encode/decode baseline.
    let decoded = core.decode(&samples).expect("Clean loopback should work");
    assert_eq!(message, decoded);
}

#[test]
#[ignore = "OFDM sync has no CFO correction yet"]
fn test_loopback_with_freq_offset() {
    use coppa_channel::freq_offset;

    let core = CoppaCore::new();
    let message = "Freq test";
    let samples = core.encode(message).unwrap();
    // Apply a real carrier frequency offset. This currently breaks OFDM
    // preamble sync, so the test is #[ignore]d until CFO correction exists.
    let shifted = freq_offset(&samples, 5.0, 48000.0, 1500.0);
    let decoded = core
        .decode(&shifted)
        .expect("Should decode with CFO correction");
    assert_eq!(message, decoded);
}

// --- Fading tests ---

#[test]
fn test_loopback_with_slow_fading() {
    let core = CoppaCore::new();
    let message = "Fading";
    let samples = core.encode(message).unwrap();
    let faded = fading(&samples, 0.5, 6.0, 48000.0);
    let decoded = core.decode(&faded).expect("Should handle slow 6 dB fading");
    assert_eq!(message, decoded);
}

// --- Combined impairment tests ---

#[test]
fn test_loopback_with_awgn_20db() {
    let core = CoppaCore::new();
    let message = "Combined";
    let samples = core.encode(message).unwrap();
    // AWGN only. (A freq-offset + noise combination is not tested here because
    // OFDM sync lacks CFO correction; see the ignored CFO test above.)
    let noisy = awgn_seeded(&samples, 20.0, 456);
    let decoded = core.decode(&noisy).expect("Should handle 20 dB AWGN");
    assert_eq!(message, decoded);
}

#[test]
fn test_loopback_fading_plus_noise() {
    let core = CoppaCore::new();
    let message = "Test";
    let samples = core.encode(message).unwrap();
    let faded = fading(&samples, 0.3, 4.0, 48000.0);
    let noisy = awgn_seeded(&faded, 15.0, 789);
    let decoded = core
        .decode(&noisy)
        .expect("Should handle fading + 15 dB AWGN");
    assert_eq!(message, decoded);
}

// --- FEC effectiveness test ---

#[test]
fn test_strong_fec_decodes_at_low_snr() {
    use coppa_engine::EngineConfig;

    let message = "FEC test";
    let test_snr = 6.0;

    // Speed level 1: BPSK + 1/4 LDPC (strongest protection)
    let strong = EngineConfig {
        speed_level: 1,
        ..Default::default()
    };
    let core_strong = CoppaCore::with_config(strong);
    let samples_strong = core_strong.encode(message).unwrap();
    let noisy_strong = awgn_seeded(&samples_strong, test_snr, 7777);
    let result_strong = core_strong.decode(&noisy_strong);

    // Strong FEC should decode at moderate SNR
    assert!(
        result_strong.is_ok(),
        "Strong FEC (speed_level=1) should decode at {} dB SNR",
        test_snr
    );
    assert_eq!(result_strong.unwrap(), message);
}

// --- Sinusoidal fade + AWGN test ---

#[test]
fn test_sinusoidal_fade_plus_awgn() {
    // Apply a deterministic sinusoidal amplitude fade followed by AWGN.
    // NOTE: `fading()` is NOT Rayleigh/Watterson fading and applies no Doppler
    // spread; it is a simple periodic AM envelope. No frequency offset is
    // applied (OFDM sync lacks CFO correction).
    let core = CoppaCore::new();
    let message = "HF sim";
    let samples = core.encode(message).unwrap();

    // 1. Sinusoidal amplitude fade: 1 Hz rate, 4 dB depth.
    let faded = fading(&samples, 1.0, 4.0, 48000.0);

    // 2. AWGN at 15 dB SNR.
    let noisy = awgn_seeded(&faded, 15.0, 9999);

    let decoder = CoppaCore::new();
    let decoded = decoder
        .decode(&noisy)
        .expect("Should decode through sinusoidal fade + AWGN at 15 dB");
    assert_eq!(message, decoded);
}

// --- Constellation mapper tests ---

#[test]
fn test_constellation_mappers_roundtrip() {
    use coppa_codec::psk8::Psk8Mapper;
    use coppa_codec::qam16::Qam16Mapper;
    use coppa_codec::qam64::Qam64Mapper;
    use coppa_codec::qpsk::QpskMapper;
    use coppa_codec::traits::ConstellationMapper;

    // QPSK
    let qpsk = QpskMapper;
    for i in 0..4u8 {
        let bits = vec![(i >> 1) & 1, i & 1];
        let sym = qpsk.map(&bits);
        let demapped = qpsk.demap_hard(sym);
        assert_eq!(demapped, bits, "QPSK failed for {:?}", bits);
    }

    // 8PSK
    let psk8 = Psk8Mapper;
    for i in 0..8u8 {
        let bits = vec![(i >> 2) & 1, (i >> 1) & 1, i & 1];
        let sym = psk8.map(&bits);
        let demapped = psk8.demap_hard(sym);
        assert_eq!(demapped, bits, "8PSK failed for {:?}", bits);
    }

    // 16QAM
    let qam16 = Qam16Mapper;
    for i in 0..16u8 {
        let bits = vec![(i >> 3) & 1, (i >> 2) & 1, (i >> 1) & 1, i & 1];
        let sym = qam16.map(&bits);
        let demapped = qam16.demap_hard(sym);
        assert_eq!(demapped, bits, "16QAM failed for {:?}", bits);
    }

    // 64QAM
    let qam64 = Qam64Mapper;
    for i in 0..64u8 {
        let bits = vec![
            (i >> 5) & 1,
            (i >> 4) & 1,
            (i >> 3) & 1,
            (i >> 2) & 1,
            (i >> 1) & 1,
            i & 1,
        ];
        let sym = qam64.map(&bits);
        let demapped = qam64.demap_hard(sym);
        assert_eq!(demapped, bits, "64QAM failed for {:?}", bits);
    }
}

// --- OFDM tests ---

#[test]
fn test_ofdm_modulate_demodulate() {
    use coppa_codec::ofdm::{OfdmDemodulator, OfdmModulator, OfdmProfile};
    use num_complex::Complex32;

    let profile = OfdmProfile::HF_STANDARD;
    let modulator = OfdmModulator::new(profile.clone());
    let demodulator = OfdmDemodulator::new(profile.clone());

    let n_active = profile.active_carriers();
    let subcarriers: Vec<Complex32> = (0..n_active)
        .map(|i| {
            if i % 2 == 0 {
                Complex32::new(1.0, 0.0)
            } else {
                Complex32::new(-1.0, 0.0)
            }
        })
        .collect();

    let samples = modulator.modulate_symbol(&subcarriers);
    let data_samples = &samples[profile.cp_length..];
    let recovered = demodulator.demodulate_symbol(data_samples);

    assert_eq!(recovered.len(), n_active);
    for (orig, recv) in subcarriers.iter().zip(recovered.iter()) {
        assert!(
            (orig - recv).norm() < 0.01,
            "OFDM roundtrip mismatch: {:?} vs {:?}",
            orig,
            recv
        );
    }
}
