//! Engine-side receive cases of the COP-190 floor session work: the engine
//! constructs a separately detected floor receive path next to its OFDM
//! paths, floor audio decodes through the seeded persistent channel with
//! zero wrong payloads, and the v1 OFDM path is unaffected. The negotiated,
<<<<<<< HEAD
//! floor-aware library controller is exercised below; the daemon and measured
//! connection bar remain open; see `docs/LIMITATIONS.md`, COP-169 section.
=======
//! floor-aware session controller (fallback, OFDM probe, connect bar) is
//! not built yet; see `docs/LIMITATIONS.md`, COP-169 section.
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b

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

<<<<<<< HEAD
// Deterministic library session regressions below do not establish a connection probability.

use coppa_engine::{Link, LinkConfig, LinkPhyMode, LinkState};
use std::time::{Duration, Instant};

// Tests need one real epoch; every subsequent deadline uses injected sample time.
#[allow(clippy::disallowed_methods)]
fn test_epoch() -> Instant {
    Instant::now()
}

/// Move actual audio in 200 ms blocks, with time derived from sample duration.
/// OFDM can be erased without erasing the independently detected floor.
fn pump(a: &mut Link, b: &mut Link, base: Instant, tick: &mut u64, seconds: u64, erase_ofdm: bool) {
    let (mut a_out, mut b_out) = (vec![0.0; 9600], vec![0.0; 9600]);
    for _ in 0..seconds * 5 {
        let now = base + Duration::from_millis(*tick * 200);
        *tick += 1;
        let (a_in, b_in) = (b_out.clone(), a_out.clone());
        a.process(now, &a_in, &mut a_out);
        b.process(now, &b_in, &mut b_out);
        if erase_ofdm {
            if a.phy_mode().0 != Some(LinkPhyMode::Floor) {
                a_out.fill(0.0);
            }
            if b.phy_mode().0 != Some(LinkPhyMode::Floor) {
                b_out.fill(0.0);
            }
        }
    }
}

#[test]
fn floor_pair_connects_and_delivers() {
    let base = test_epoch();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
    b.set_listening(true);
    a.connect("N0BBB", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 60, true);
    assert_eq!(a.state(), LinkState::Connected);
    assert_eq!(b.state(), LinkState::Connected);
    let payload = b"a message longer than the floor's nineteen application bytes";
    a.send(payload).unwrap();
    pump(&mut a, &mut b, base, &mut tick, 100, true);
    let mut received = [0; 100];
    let n = b.recv(&mut received);
    assert_eq!(&received[..n], payload);
    assert_eq!(a.pending_tx_bytes(), 0);
    assert_eq!(b.recv(&mut received), 0);
}

#[test]
fn floor_v1_peer_stays_ofdm() {
    let base = test_epoch();
    let mut config = LinkConfig::new("N0BBB");
    config.robust_floor = false;
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(config).unwrap();
    b.set_listening(true);
    a.connect("N0BBB", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 15, false);
    assert_eq!(a.state(), LinkState::Connected);
    assert_eq!(b.state(), LinkState::Connected);
    assert!(matches!(a.phy_mode().0, Some(LinkPhyMode::Ofdm { .. })));
    a.send(b"legacy").unwrap();
    pump(&mut a, &mut b, base, &mut tick, 15, false);
    let mut bytes = [0; 6];
    assert_eq!(b.recv(&mut bytes), 6);
    assert_eq!(&bytes, b"legacy");
}

#[test]
fn floor_phy_mode_reporting() {
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    assert_eq!(a.phy_mode(), (None, None));
    let now = test_epoch();
    a.connect("N0BBB", now).unwrap();
    a.process(now, &[], &mut [0.0; 9600]);
    assert!(matches!(a.phy_mode().0, Some(LinkPhyMode::Ofdm { .. })));
    assert_eq!(a.phy_mode().1, None);
    a.disconnect(now).unwrap();
    assert_eq!(a.phy_mode(), (None, None));
}

#[test]
fn floor_five_ack_probe() {
    for lose_probe in [false, true] {
        let base = test_epoch();
        let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
        let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
        b.set_listening(true);
        a.connect("N0BBB", base).unwrap();
        let mut tick = 0;
        pump(&mut a, &mut b, base, &mut tick, 60, true);
        assert_eq!(b.state(), LinkState::Connected);
        let payload: Vec<u8> = (0..19 * 7).map(|i| i as u8).collect();
        a.send(&payload).unwrap();
        let (mut a_out, mut b_out) = (vec![0.0; 9600], vec![0.0; 9600]);
        let mut saw_probe = false;
        let mut saw_return = false;
        let mut last_count = a.frames_transmitted();
        for _ in 0..240 * 5 {
            let now = base + Duration::from_millis(tick * 200);
            tick += 1;
            let (a_in, b_in) = (b_out.clone(), a_out.clone());
            a.process(now, &a_in, &mut a_out);
            b.process(now, &b_in, &mut b_out);
            if a.frames_transmitted() != last_count {
                last_count = a.frames_transmitted();
                if matches!(a.phy_mode().0, Some(LinkPhyMode::Ofdm { level: 1 })) {
                    saw_probe = true;
                }
                if saw_probe && a.phy_mode().0 == Some(LinkPhyMode::Floor) {
                    saw_return = true;
                }
            }
            if lose_probe {
                if a.phy_mode().0 != Some(LinkPhyMode::Floor) {
                    a_out.fill(0.0);
                }
                if b.phy_mode().0 != Some(LinkPhyMode::Floor) {
                    b_out.fill(0.0);
                }
            }
            if a.pending_tx_bytes() == 0 && b.received_len() == payload.len() {
                break;
            }
        }
        assert!(saw_probe, "five new ACKs must permit a probe");
        assert_eq!(saw_return, lose_probe);
        let mut received = vec![0; payload.len() + 1];
        assert_eq!(b.recv(&mut received), payload.len());
        assert_eq!(&received[..payload.len()], payload);
        assert_eq!(b.received_len(), 0);
        assert_eq!(a.pending_tx_bytes(), 0);
    }
}

#[test]
fn floor_simultaneous_calls() {
    let base = test_epoch();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
    // Outgoing calls must converge even when unsolicited incoming calls are disabled.
    a.connect("N0BBB", base).unwrap();
    b.connect("N0AAA", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 100, true);
    assert_eq!(a.state(), LinkState::Connected);
    assert_eq!(b.state(), LinkState::Connected);
    for link in [&mut a, &mut b] {
        assert_eq!(
            std::iter::from_fn(|| link.poll_event())
                .filter(|e| matches!(e, coppa_engine::LinkEvent::Connected { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn floor_duplicate_handshake() {
    use coppa_protocol::{
        mac::{Callsign, MacFrameType, MacPdu},
        session::LinkCapabilities,
    };
    let base = test_epoch();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
    b.set_listening(true);
    a.connect("N0BBB", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 60, true);
    let caps = LinkCapabilities {
        robust_floor: true,
        ..Default::default()
    };
    let core = CoppaCore::new();
    // Replay complete, validated handshake audio after establishment.
    for (kind, dest, src, receiver) in [
        (MacFrameType::ConnectReq, "N0BBB", "N0AAA", &mut b),
        (MacFrameType::ConnectAck, "N0AAA", "N0BBB", &mut a),
    ] {
        let pdu = MacPdu::new(
            kind,
            Callsign::new(dest).unwrap(),
            Callsign::new(src).unwrap(),
            0,
            caps.to_bytes(),
        );
        let mut audio = core.floor().encode(&pdu.to_bytes()).unwrap();
        audio.extend(vec![0.0; 2 * SAMPLE_RATE]);
        for chunk in audio.chunks(9600) {
            receiver.process(
                base + Duration::from_millis(tick * 200),
                chunk,
                &mut [0.0; 9600],
            );
            tick += 1;
        }
        assert_eq!(receiver.state(), LinkState::Connected);
        assert_eq!(
            std::iter::from_fn(|| receiver.poll_event())
                .filter(|e| matches!(e, coppa_engine::LinkEvent::Connected { .. }))
                .count(),
            1
        );
        assert_eq!(receiver.received_len(), 0);
    }
}
=======
// The full floor session controller (negotiation, fallback, OFDM probe and
// the release-mode -6 dB 9-of-10 connect bar) is deferred with Phase 2 of
// the COP-190 plan; this file carries the receive cases that are built.
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b
