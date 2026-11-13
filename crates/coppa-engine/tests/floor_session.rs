//! Engine-side receive cases of the COP-190 floor session work: the engine
//! constructs a separately detected floor receive path next to its OFDM
//! paths, floor audio decodes through the seeded persistent channel with
//! zero wrong payloads, and the v1 OFDM path is unaffected. The negotiated,
//! floor-aware session controller (fallback, OFDM probe, connect bar) is
//! not built yet; see `docs/LIMITATIONS.md`, COP-169 section.

use coppa_channel::persistent::{PersistentChannel, SnrProfile};
use coppa_engine::CoppaCore;

const SAMPLE_RATE: usize = 48_000;

#[test]
fn engine_constructs_a_separately_detected_floor_receive_path() {
    let mut core = CoppaCore::new();
    let payload = b"engine floor path";
    let audio = core.floor().encode(payload).unwrap();
    assert_eq!(audio.len(), 8 * SAMPLE_RATE);
    assert_eq!(core.floor().receive(&audio).unwrap(), payload.to_vec());
    // The continuous detector delivers the same frame exactly once, from
    // arbitrary chunk shapes.
    let mut stream_audio = vec![0.0f32; SAMPLE_RATE];
    stream_audio.extend(&audio);
    stream_audio.extend(vec![0.0; 2 * SAMPLE_RATE]);
    let mut delivered = Vec::new();
    for chunk in stream_audio.chunks(317) {
        delivered.extend(core.push_floor_samples(chunk));
    }
    assert_eq!(delivered, [payload.to_vec()]);
    // The OFDM transceiver still decodes its own v1 frames (regression
    // guard for the shared-engine case).
    let ofdm_audio = core.encode("v1").expect("v1 transmit");
    assert_eq!(core.decode(&ofdm_audio).expect("v1 decode"), "v1");
}

#[test]
fn floor_audio_through_the_persistent_channel_decodes_exactly() {
    let mut core = CoppaCore::new();
    let payload = b"persistent engine floor";
    let clean = core.floor().encode(payload).unwrap();
    let active_power: f32 = clean.iter().map(|x| x * x).sum::<f32>() / clean.len() as f32;
    let mut channel = PersistentChannel::new(
        SAMPLE_RATE as f32,
        None,
        Some(SnrProfile::constant(-6.0)),
        active_power,
        0x1691_0000,
        1,
    );
    let mut received = Vec::new();
    let mut position = 0;
    while position < clean.len() {
        let end = (position + 9600).min(clean.len());
        let block = channel.process(&clean[position..end]);
        received.extend(core.push_floor_samples(&block));
        position = end;
    }
    received.extend(core.push_floor_samples(&vec![0.0; 4 * SAMPLE_RATE]));
    assert_eq!(received, [payload.to_vec()]);
}

// The full floor session controller (negotiation, fallback, OFDM probe and
// the release-mode -6 dB 9-of-10 connect bar) is deferred with Phase 2 of
// the COP-190 plan; this file carries the receive cases that are built.
