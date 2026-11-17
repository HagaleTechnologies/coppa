//! KISS framing, from the public KISS TNC protocol paper.

use std::time::{Duration, Instant};

pub const FEND: u8 = 0xc0;
pub const FESC: u8 = 0xdb;
pub const TFEND: u8 = 0xdc;
pub const TFESC: u8 = 0xdd;

pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![FEND, 0];
    for &b in payload {
        match b {
            FEND => out.extend_from_slice(&[FESC, TFEND]),
            FESC => out.extend_from_slice(&[FESC, TFESC]),
            _ => out.push(b),
        }
    }
    out.push(FEND);
    out
}

pub fn decode(frame: &[u8]) -> Option<Vec<u8>> {
    let bytes = frame.strip_prefix(&[FEND, 0])?.strip_suffix(&[FEND])?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == FESC {
            i += 1;
            out.push(match *bytes.get(i)? {
                TFEND => FEND,
                TFESC => FESC,
                _ => return None,
            });
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    Some(out)
}

pub fn frames(payload: &[u8], datagram_bytes: usize) -> Vec<Vec<u8>> {
    if datagram_bytes == 0 {
        return Vec::new();
    }
    payload.chunks(datagram_bytes).map(frame).collect()
}

pub fn drain_frames(wire: &mut Vec<u8>, payload: &mut Vec<u8>) -> usize {
    let datagrams = drain_datagrams(wire);
    for datagram in &datagrams {
        payload.extend_from_slice(datagram);
    }
    datagrams.len()
}

/// Removes every complete frame from `wire` and returns each one's payload.
pub fn drain_datagrams(wire: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut datagrams = Vec::new();
    loop {
        let Some(start) = wire.iter().position(|&b| b == FEND) else {
            wire.clear();
            break;
        };
        if start > 0 {
            wire.drain(..start);
        }
        let Some(end) = wire[1..].iter().position(|&b| b == FEND).map(|p| p + 1) else {
            break;
        };
        if let Some(bytes) = decode(&wire[..=end]) {
            datagrams.push(bytes);
        }
        wire.drain(..=end);
    }
    datagrams
}

/// Most datagrams left with the TNC that the receiver has not accounted for.
/// KISS has no flow control. coppad queues at most eight datagrams to transmit
/// and drops the newest beyond that (`docs/RETICULUM.md`), so eight
/// unaccounted for never overflow its queue. A TNC with a shorter queue still
/// can, and so can a presumed loss (see [`PRESUME_LOST`]).
pub const WINDOW: usize = 8;

/// How long the oldest outstanding datagram may go unaccounted for, with no
/// datagram received, before it is presumed lost and its slot reused. One
/// 900-byte datagram at coppad's level 2 is about 11 s on air (scaled from
/// the 5.99 s for 500 bytes in `docs/RETICULUM.md`), so while coppad keeps
/// transmitting it has drained one datagram for each slot this frees. A TNC
/// that pauses longer than this, or takes longer to send one datagram, can
/// overflow.
pub const PRESUME_LOST: Duration = Duration::from_secs(60);

/// Paces KISS datagrams on receiver progress, so a trial measures channel
/// loss and not an overflowed TNC input queue.
///
/// The TNC transmits its queue in order, so receiving datagram `j` accounts
/// for every earlier one: each was received or lost on the channel.
pub struct Pacer<'a> {
    chunks: Vec<&'a [u8]>,
    window: usize,
    presume_lost: Duration,
    sent: usize,
    resolved: usize,
    since: Instant,
}

impl<'a> Pacer<'a> {
    pub fn new(
        payload: &'a [u8],
        datagram_bytes: usize,
        window: usize,
        presume_lost: Duration,
        now: Instant,
    ) -> Self {
        let chunks = if datagram_bytes == 0 {
            Vec::new()
        } else {
            payload.chunks(datagram_bytes).collect()
        };
        Self {
            chunks,
            window,
            presume_lost,
            sent: 0,
            resolved: 0,
            since: now,
        }
    }

    /// The KISS frames to write now, so that at most `window` are outstanding.
    pub fn due(&mut self, now: Instant) -> Vec<Vec<u8>> {
        if self.sent > self.resolved && now.duration_since(self.since) >= self.presume_lost {
            self.resolved += 1;
            self.since = now;
        }
        if self.sent == self.resolved {
            self.since = now;
        }
        let end = (self.resolved + self.window).min(self.chunks.len());
        let frames = self.chunks[self.sent..end]
            .iter()
            .map(|chunk| frame(chunk))
            .collect();
        self.sent = end;
        frames
    }

    /// Accounts for one received datagram's payload. One that matches no
    /// outstanding datagram, such as one already presumed lost, frees nothing.
    pub fn received(&mut self, datagram: &[u8], now: Instant) {
        if let Some(i) = (self.resolved..self.sent).find(|&i| self.chunks[i] == datagram) {
            self.resolved = i + 1;
        }
        self.since = now;
    }
}

pub fn lost_datagrams(size_bytes: usize, datagram_bytes: usize, received: usize) -> usize {
    if datagram_bytes == 0 {
        return 0;
    }
    size_bytes.div_ceil(datagram_bytes).saturating_sub(received)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fend_fesc_escaping_round_trips() {
        let p = [1, FEND, 2, FESC];
        assert_eq!(decode(&frame(&p)).unwrap(), p);
    }
    #[test]
    fn payload_splits_at_datagram_bytes() {
        assert_eq!(frames(&[0; 2000], 900).len(), 3);
    }
    #[test]
    fn stream_decodes_multiple_datagrams() {
        let mut wire = frame(b"ab");
        wire.extend(frame(b"cd"));
        let mut payload = Vec::new();
        assert_eq!(drain_frames(&mut wire, &mut payload), 2);
        assert_eq!(payload, b"abcd");
    }
    #[test]
    fn lost_datagrams_count_as_drops() {
        assert_eq!(lost_datagrams(2048, 900, 2), 1);
        assert_eq!(lost_datagrams(2048, 900, 3), 0);
    }

    #[test]
    fn a_datagram_received_after_its_presumed_loss_frees_no_slot() {
        let payload = [1, 2, 3, 4];
        let t0 = Instant::now();
        let mut pacer = Pacer::new(&payload, 1, 2, PRESUME_LOST, t0);
        assert_eq!(pacer.due(t0).len(), 2);
        // Nothing received for PRESUME_LOST: datagram 0 is presumed lost.
        assert_eq!(pacer.due(t0 + PRESUME_LOST).len(), 1);
        // Datagram 0 was only late. The TNC still holds 1 and 2.
        let late = t0 + PRESUME_LOST + Duration::from_secs(1);
        pacer.received(&[1], late);
        assert!(pacer.due(late).is_empty());
    }

    /// One 900-byte datagram's airtime at coppad's level 2, rounded up.
    const AIRTIME: Duration = Duration::from_secs(11);

    /// coppad's KISS input as `docs/RETICULUM.md` describes it: one datagram
    /// on the air, at most eight waiting, the newest dropped when full.
    #[derive(Default)]
    struct Tnc {
        airing: Option<(Vec<u8>, Duration)>,
        queue: std::collections::VecDeque<Vec<u8>>,
        dropped: usize,
    }

    impl Tnc {
        fn accept(&mut self, frame: Vec<u8>, now: Duration) {
            if self.airing.is_none() {
                self.airing = Some((frame, now + AIRTIME));
            } else if self.queue.len() == 8 {
                self.dropped += 1;
            } else {
                self.queue.push_back(frame);
            }
        }

        /// The frame whose airtime ended by `now`, if any.
        fn tick(&mut self, now: Duration) -> Option<Vec<u8>> {
            if !matches!(self.airing, Some((_, end)) if end <= now) {
                return None;
            }
            let (frame, _) = self.airing.take()?;
            if let Some(next) = self.queue.pop_front() {
                self.airing = Some((next, now + AIRTIME));
            }
            Some(frame)
        }
    }

    #[test]
    fn paced_16_kib_never_overflows_coppads_queue() {
        let payload: Vec<u8> = (0..16384u64)
            .map(|i| (0x1700_0000u64.wrapping_add(i) & 0xff) as u8)
            .collect();
        let mut unpaced = Tnc::default();
        for frame in frames(&payload, 900) {
            unpaced.accept(frame, Duration::ZERO);
        }
        assert_eq!(
            unpaced.dropped, 10,
            "19 datagrams sent at once overflow the queue"
        );

        let all: Vec<usize> = (0..19).collect();
        for lost in [vec![], vec![3, 4, 18], all] {
            let base = Instant::now();
            let mut pacer = Pacer::new(&payload, 900, WINDOW, PRESUME_LOST, base);
            let mut tnc = Tnc::default();
            let mut aired = 0;
            let mut received = Vec::new();
            let mut t = Duration::ZERO;
            while t < Duration::from_secs(1800) {
                for frame in pacer.due(base + t) {
                    tnc.accept(frame, t);
                }
                if let Some(frame) = tnc.tick(t) {
                    if !lost.contains(&aired) {
                        let datagram = decode(&frame).unwrap();
                        pacer.received(&datagram, base + t);
                        received.push(datagram);
                    }
                    aired += 1;
                }
                t += Duration::from_millis(100);
            }
            assert_eq!(tnc.dropped, 0, "lost {lost:?}: the TNC queue overflowed");
            assert_eq!(
                aired, 19,
                "lost {lost:?}: not every datagram reached the air"
            );
            assert_eq!(received.len(), 19 - lost.len(), "lost {lost:?}");
            if lost.is_empty() {
                assert_eq!(received.concat(), payload);
            }
        }
    }
}
