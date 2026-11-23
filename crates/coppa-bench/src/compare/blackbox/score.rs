//! What a black-box trial writes and how its delivery is scored (COP-227).

use crc::{Crc, CRC_32_ISO_HDLC};

/// The bytes a trial writes (COP-170 decision 8): `StdRng` seeded with
/// `derive_seed(seed, 0, 2)`, the link rows' payload for the same seed.
#[must_use]
pub fn trial_payload(seed: u64, size: usize) -> Vec<u8> {
    super::super::link_stack::payload(seed, size)
}

/// The CSV's `payload_crc32` value: CRC-32/ISO-HDLC of the generated trial
/// payload, as `0x` and eight lower-case hex digits.
#[must_use]
pub fn payload_crc32(payload: &[u8]) -> String {
    format!(
        "0x{:08x}",
        Crc::<u32>::new(&CRC_32_ISO_HDLC).checksum(payload)
    )
}

/// `delivered_bytes` for a stream adapter: payload bytes received in order
/// before the first byte that differs, at most the payload size.
#[must_use]
pub fn stream_delivered(received: &[u8], payload: &[u8]) -> usize {
    received
        .iter()
        .zip(payload)
        .take_while(|(a, b)| a == b)
        .count()
}

/// `delivered_bytes` for the KISS adapter, and the number of intact
/// datagrams: each received datagram equal to a not-yet-counted payload
/// chunk of `datagram_bytes` counts that chunk once. A corrupt, foreign or
/// repeated datagram adds nothing when payload chunks have distinct bytes,
/// as required by the KISS trial before sending.
#[must_use]
pub fn datagram_delivered(
    datagrams: &[Vec<u8>],
    payload: &[u8],
    datagram_bytes: usize,
) -> (usize, usize) {
    if datagram_bytes == 0 {
        return (0, 0);
    }
    let chunks: Vec<&[u8]> = payload.chunks(datagram_bytes).collect();
    let mut counted = vec![false; chunks.len()];
    let mut bytes = 0;
    let mut intact = 0;
    for datagram in datagrams {
        if let Some(i) =
            (0..chunks.len()).find(|&i| !counted[i] && chunks[i] == datagram.as_slice())
        {
            counted[i] = true;
            bytes += chunks[i].len();
            intact += 1;
        }
    }
    (bytes, intact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trial_payload_is_the_link_rows_seeded_bytes() {
        use rand::{RngExt, SeedableRng};
        let seed = 0x1700_0000;
        let payload = trial_payload(seed, 16384);
        // COP-170 decision 8, built here rather than through `link_stack`.
        let mut rng =
            rand::rngs::StdRng::seed_from_u64(coppa_channel::persistent::derive_seed(seed, 0, 2));
        let expected: Vec<u8> = (0..16384).map(|_| rng.random::<u8>()).collect();
        assert_eq!(payload, expected);
        // coppad's HF_STANDARD compresses each session chunk of up to 4096
        // bytes; a compressible payload would overstate a trial's net B/min.
        let core = coppa_engine::CoppaCore::with_config(coppa_engine::EngineConfig::from_profile(
            &coppa_engine::profiles::HF_STANDARD,
        ));
        for chunk in payload.chunks(4096) {
            assert!(core.encoded_payload_len(chunk) >= chunk.len());
        }
    }

    #[test]
    fn payload_crc32_pins_the_published_seeds() {
        for (seed, expected) in [
            (0x1700_0000, "0x6887c6a5"),
            (0x1700_0001, "0xe6429c02"),
            (0x1700_0002, "0x7991e53c"),
            (0x1712_0000, "0xb3f15e0c"),
            (0x1712_0001, "0xd67f050a"),
            (0x1712_0002, "0x01d16714"),
        ] {
            assert_eq!(
                payload_crc32(&trial_payload(seed, 2048)),
                expected,
                "seed 0x{seed:x}"
            );
        }
        // The COP-214 ramp the ramp-era rows wrote has its own value.
        let ramp: Vec<u8> = (0..2048u64)
            .map(|i| ((0x1700_0000u64 + i) & 0xff) as u8)
            .collect();
        assert_eq!(payload_crc32(&ramp), "0x9f5edd58");
        assert_ne!(
            payload_crc32(&ramp),
            payload_crc32(&trial_payload(0x1700_0000, 2048))
        );
    }

    #[test]
    fn stream_scoring_counts_the_exact_prefix_only() {
        let payload = trial_payload(0x1700_0000, 2048);
        assert_eq!(stream_delivered(&payload, &payload), 2048);
        assert_eq!(stream_delivered(&payload[..515], &payload), 515);
        let mut wrong_at_100 = payload.clone();
        wrong_at_100[100] ^= 0xff;
        assert_eq!(stream_delivered(&wrong_at_100, &payload), 100);
        let mut wrong_first = payload.clone();
        wrong_first[0] ^= 0xff;
        assert_eq!(stream_delivered(&wrong_first, &payload), 0);
        assert_eq!(stream_delivered(&[], &payload), 0);
        let mut extra = payload.clone();
        extra.extend_from_slice(&[0u8; 10]);
        assert_eq!(stream_delivered(&extra, &payload), 2048);
        // A resent span: the bytes after the repeat start no longer match.
        let mut resent = payload[..1000].to_vec();
        resent.extend_from_slice(&payload[500..]);
        assert_eq!(stream_delivered(&resent, &payload), 1000);
    }

    #[test]
    fn datagram_scoring_counts_each_intact_datagram_once() {
        let payload = trial_payload(0x1700_0000, 2048);
        let all: Vec<Vec<u8>> = payload.chunks(256).map(<[u8]>::to_vec).collect();
        assert_eq!(datagram_delivered(&all, &payload, 256), (2048, 8));

        let mut lost = all.clone();
        lost.remove(3);
        assert_eq!(datagram_delivered(&lost, &payload, 256), (1792, 7));
        lost.insert(3, all[0].clone());
        assert_eq!(datagram_delivered(&lost, &payload, 256), (1792, 7));

        let mut flipped = all.clone();
        flipped[2][17] ^= 0x01;
        assert_eq!(datagram_delivered(&flipped, &payload, 256), (1792, 7));

        let mut twice = all.clone();
        twice.push(all[5].clone());
        assert_eq!(datagram_delivered(&twice, &payload, 256), (2048, 8));

        let mut reordered = all.clone();
        reordered.reverse();
        assert_eq!(datagram_delivered(&reordered, &payload, 256), (2048, 8));

        let mut foreign = all.clone();
        foreign.push(vec![0x55; 256]);
        assert_eq!(datagram_delivered(&foreign, &payload, 256), (2048, 8));

        let short = trial_payload(0x1700_0000, 2000);
        let short_all: Vec<Vec<u8>> = short.chunks(256).map(<[u8]>::to_vec).collect();
        assert_eq!(short_all.last().unwrap().len(), 208);
        assert_eq!(datagram_delivered(&short_all, &short, 256), (2000, 8));
        assert_eq!(
            datagram_delivered(&short_all[7..], &short, 256),
            (208, 1),
            "the short last datagram counts as its own 208 bytes"
        );

        assert_eq!(datagram_delivered(&all, &payload, 0), (0, 0));
    }
}
