//! Production floor receive path (COP-190): separately detected from the
//! OFDM path, chunk-shape agnostic, bounded in memory, and silent on
//! truncated, corrupt, nonfinite and pure-noise input.

use coppa_channel::persistent::{PersistentChannel, SnrProfile};
use coppa_channel::{awgn_ref_seeded, frequency_shift, ssb_filter};
use coppa_codec::floor::SAMPLE_RATE;
use coppa_protocol::modem::floor::{
    FloorError, FloorFrame, FloorStream, FRAME_SAMPLES, MAX_PAYLOAD,
};

#[test]
fn decodes_at_unknown_offsets_and_through_arbitrary_chunks() {
    let floor = FloorFrame::new();
    let sent: Vec<_> = (0..MAX_PAYLOAD as u8).collect();
    for offset in [1, 119, 481, 1907] {
        let mut audio = vec![0.0; offset];
        audio.extend(floor.encode(&sent).unwrap());
        audio.extend(vec![0.0; 960]);
        assert_eq!(floor.receive(&audio).unwrap(), sent);
    }
    for chunk_size in [317, 48_000, FRAME_SAMPLES + 13] {
        let mut audio = vec![0.0; 2 * SAMPLE_RATE + 137];
        audio.extend(floor.encode(b"first").unwrap());
        audio.extend(vec![0.0; 719]);
        audio.extend(floor.encode(b"second").unwrap());
        audio.extend(vec![0.0; 2 * SAMPLE_RATE]);
        let mut stream = FloorStream::default();
        let mut got = Vec::new();
        for chunk in audio.chunks(chunk_size) {
            got.extend(stream.push(chunk));
            assert!(stream.retained_samples() <= coppa_protocol::modem::floor::CAPTURE_LIMIT);
        }
        assert_eq!(got, [b"first".to_vec(), b"second".to_vec()]);
        assert!(stream.decode_attempts() <= (audio.len() / SAMPLE_RATE) as u64);
    }
}

#[test]
fn rejects_truncated_corrupt_and_nonfinite_input() {
    let floor = FloorFrame::new();
    let audio = floor.encode(b"incomplete").unwrap();
    // Truncated by one symbol.
    assert_eq!(
        floor.receive(&audio[..audio.len() - 960]).unwrap_err(),
        FloorError::Payload
    );
    // Corrupt header: the deepest reached stage is the header.
    let mut corrupt = floor.encode(b"bad header").unwrap();
    corrupt[16 * 960..(16 + 32) * 960].fill(0.0);
    assert_eq!(floor.receive(&corrupt).unwrap_err(), FloorError::Header);
    // Nonfinite audio breaks acquisition outright.
    let mut nan = floor.encode(b"nan").unwrap();
    nan[0] = f32::NAN;
    assert_eq!(floor.receive(&nan).unwrap_err(), FloorError::Acquisition);
    assert_eq!(
        floor.receive(&[f32::NAN, f32::INFINITY]).unwrap_err(),
        FloorError::Acquisition
    );
    // Oversized capture is refused rather than searched.
    assert_eq!(
        floor
            .receive(&vec![0.0; coppa_protocol::modem::floor::CAPTURE_LIMIT + 1])
            .unwrap_err(),
        FloorError::Acquisition
    );
    // The streaming receiver recovers after the corrupt input.
    let mut stream = FloorStream::default();
    assert!(stream.push(&corrupt).is_empty());
    assert!(stream.push(&vec![0.0; SAMPLE_RATE]).is_empty());
    let mut good = floor.encode(b"recovered").unwrap();
    good.extend(vec![0.0; SAMPLE_RATE]);
    assert_eq!(stream.push(&good), [b"recovered".to_vec()]);
}

#[test]
fn cfo_search_decodes_unknown_offset_in_batch_and_chunks() {
    let floor = FloorFrame::new();
    let payload = b"CFO acquisition regression";
    let clean = floor.encode(payload).unwrap();
    for cfo in [-150.0, -75.0, 75.0, 150.0] {
        let mut audio = vec![0.0; 137];
        audio.extend(frequency_shift(&clean, cfo, SAMPLE_RATE as f32));
        assert_eq!(floor.receive(&audio), Ok(payload.to_vec()), "CFO {cfo}");
        audio.extend(vec![0.0; SAMPLE_RATE]);
        let mut stream = FloorStream::default();
        let mut delivered = Vec::new();
        for chunk in audio.chunks(317) {
            delivered.extend(stream.push(chunk));
            assert!(stream.retained_samples() <= coppa_protocol::modem::floor::CAPTURE_LIMIT);
        }
        assert_eq!(delivered, [payload.to_vec()], "stream CFO {cfo}");
    }
}

#[test]
fn rejects_noise_with_no_wrong_payload() {
    let floor = FloorFrame::new();
    let noise = awgn_ref_seeded(
        &vec![0.0; FRAME_SAMPLES + 960],
        -6.0,
        0.5,
        SAMPLE_RATE as f32,
        123,
    );
    assert!(floor.receive(&noise).is_err());
    let mut stream = FloorStream::default();
    for _ in 0..12 {
        assert!(stream
            .push(&awgn_ref_seeded(
                &vec![0.0; SAMPLE_RATE],
                -6.0,
                0.5,
                SAMPLE_RATE as f32,
                123,
            ))
            .is_empty());
    }
    assert!(stream.retained_samples() <= coppa_protocol::modem::floor::CAPTURE_LIMIT);
}

/// The floor receive path is separately detected from OFDM: floor audio
/// through the seeded persistent channel decodes with zero wrong payloads.
#[test]
fn decodes_floor_audio_through_the_persistent_channel() {
    let floor = FloorFrame::new();
    let payload = b"persistent channel floor";
    let clean = floor.encode(payload).unwrap();
    let active_power: f32 = clean.iter().map(|x| x * x).sum::<f32>() / clean.len() as f32;
    let mut channel = PersistentChannel::new(
        SAMPLE_RATE as f32,
        None,
        Some(SnrProfile::constant(-6.0)),
        active_power,
        0x00C0FFEE,
        1,
    );
    let mut stream = FloorStream::default();
    let mut delivered = Vec::new();
    let mut position = 0;
    while position < clean.len() {
        let end = (position + 9600).min(clean.len());
        let block = channel.process(&clean[position..end]);
        delivered.extend(stream.push(&block));
        position = end;
    }
    delivered.extend(stream.push(&vec![0.0; 4 * SAMPLE_RATE]));
    assert_eq!(delivered, [payload.to_vec()]);
}

// Long scientific check, run explicitly in release mode.
#[test]
#[ignore = "300 seconds each of three noise inputs"]
fn three_hundred_seconds_of_noise_delivers_nothing() {
    for kind in 0..3 {
        let mut stream = FloorStream::default();
        for second in 0..300 {
            let mut noise = awgn_ref_seeded(
                &vec![0.0; SAMPLE_RATE],
                -6.0,
                0.5,
                SAMPLE_RATE as f32,
                0x00C0FFEE + second,
            );
            if kind == 1 {
                noise = ssb_filter(&noise, SAMPLE_RATE as f32);
            } else if kind == 2 {
                for (n, x) in noise.iter_mut().enumerate() {
                    *x += (std::f32::consts::TAU * 1000.0 * n as f32 / SAMPLE_RATE as f32).cos();
                }
            }
            assert!(
                stream.push(&noise).is_empty(),
                "kind={kind}, second={second}"
            );
            assert!(stream.retained_samples() <= coppa_protocol::modem::floor::CAPTURE_LIMIT);
            assert!(stream.decode_attempts() <= second + 1);
        }
        assert_eq!(stream.decode_attempts(), 293);
    }
}
