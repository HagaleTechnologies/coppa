//! Session burst construction. Wire rules live in SPEC section 22.
use anyhow::{Context, Result};
use coppa_engine::TransmitHalf;
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
use coppa_protocol::outer_code::{encode_block, harq_key, shard_len_for_level};
use coppa_protocol::transport::TransportPdu;

#[derive(Clone, Copy)]
pub(crate) struct OuterTx {
    pub level: u8,
    pub packets_len: usize,
}

/// Encode one whole outer-code burst through the taken transmit half
/// (COP-258): the event loop runs this on a blocking worker while it keeps
/// draining the audio input ring. Identical bytes and errors to the
/// pre-COP-258 `CoppaCore` path.
pub(crate) fn encode_burst(
    half: &TransmitHalf,
    addresses: (&Callsign, &Callsign, u8),
    (stream, seq): (u8, u8),
    data: &[u8],
    level: u8,
) -> Result<(Vec<f32>, usize)> {
    let (remote, local, ssid) = addresses;
    // COP-121: the transport `session_id` nibble is the segment's stream.
    let segment = TransportPdu::new_reliable(stream, seq, 0, data.to_vec()).to_bytes();
    let shard_len = shard_len_for_level(level).context("invalid outer packet level")?;
    let packets = encode_block(seq, &segment, shard_len)?;
    let mut burst = Vec::new();
    let mut packet_len = 0;
    for packet in packets {
        let frame = MacPdu::new(
            MacFrameType::OuterData,
            remote.clone(),
            local.clone(),
            ssid,
            packet.to_bytes()?,
        );
        let samples =
            half.encode_packet_with_seq(&frame.to_bytes(), level, harq_key(seq, packet.index))?;
        packet_len = samples.len();
        burst.extend(samples);
    }
    Ok((burst, packet_len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_codec::ofdm::CoppaProfile;
    use coppa_engine::config::{EngineConfig, OfdmBand};
    use coppa_engine::CoppaCore;
    use coppa_protocol::modem::streaming::StreamingReceiver;
    use coppa_protocol::outer_code::{InsertOutcome, OuterBlockRx, OuterPacket, OUTER_N};

    #[test]
    fn rs_burst_has_twelve_distinct_keys_and_rebuilds_the_transport_segment() {
        let mut engine = CoppaCore::with_config(EngineConfig {
            speed_level: 2,
            ofdm_band: OfdmBand::Hf,
            compression_enabled: true,
            ..Default::default()
        });
        let local = Callsign::new("N0AAA").unwrap();
        let remote = Callsign::new("N0BBB").unwrap();
        let host: Vec<_> = (0..587).map(|i| (i % 251) as u8).collect();
        // COP-258: the burst encoder takes the movable transmit half and
        // returns it, exactly as the daemon's worker does.
        let half = engine.take_transmit().unwrap();
        let (burst, packet_len) =
            encode_burst(&half, (&remote, &local, 3), (0, 19), &host, 2).unwrap();
        engine.put_transmit(half);
        assert_eq!(burst.len(), OUTER_N * packet_len);
        let mut samples = vec![0.0; 8192];
        samples.extend(burst);
        samples.extend(vec![0.0; 24_000]);
        let mut receiver = StreamingReceiver::new(CoppaProfile::hf_standard(), 1);
        let frames: Vec<_> = samples
            .chunks(960)
            .flat_map(|chunk| receiver.push_samples(chunk))
            .collect();
        assert_eq!(frames.len(), OUTER_N);
        let mut block = OuterBlockRx::default();
        let mut rebuilt = None;
        for (i, frame) in frames.into_iter().enumerate() {
            assert_eq!(frame.header.seq_num, harq_key(19, i as u8));
            let mac = MacPdu::from_bytes(&frame.payload).unwrap();
            assert_eq!(mac.frame_type, MacFrameType::OuterData);
            assert_eq!(mac.dest, remote);
            assert_eq!(mac.src, local);
            let packet = OuterPacket::from_bytes(&mac.payload).unwrap();
            assert_eq!(packet.index as usize, i);
            if let InsertOutcome::Rebuilt(segment) = block.insert(&packet) {
                rebuilt = Some(segment);
            }
        }
        assert_eq!(
            rebuilt.unwrap(),
            TransportPdu::new_reliable(0, 19, 0, host).to_bytes()
        );
    }

    #[test]
    fn rs_retransmit_is_byte_identical_at_the_block_level() {
        let mut engine = CoppaCore::with_config(EngineConfig {
            speed_level: 2,
            ofdm_band: OfdmBand::Hf,
            ..Default::default()
        });
        let local = Callsign::new("N0AAA").unwrap();
        let remote = Callsign::new("N0BBB").unwrap();
        // COP-258: restore the half before the speed change so the engine
        // reconfigures a resident half, then take it again for the retry.
        let half = engine.take_transmit().unwrap();
        let original =
            encode_burst(&half, (&remote, &local, 0), (0, 255), b"pinned block", 2).unwrap();
        engine.put_transmit(half);
        engine.set_speed_level(5).unwrap();
        let half = engine.take_transmit().unwrap();
        let retransmit =
            encode_burst(&half, (&remote, &local, 0), (0, 255), b"pinned block", 2).unwrap();
        engine.put_transmit(half);
        assert_eq!(original, retransmit);
    }
}
