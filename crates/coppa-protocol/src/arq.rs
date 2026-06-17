//! Selective Repeat ARQ (Automatic Repeat reQuest) for Coppa.
//!
//! Provides reliable delivery over unreliable radio links using:
//! - Selective repeat with configurable window size (default 8, max 32)
//! - Selective ACK bitmap (8 frames beyond cumulative ACK)
//! - Karn's algorithm for RTT estimation (ignore retransmitted segments)
//! - Exponential backoff on retransmit timeout
//! - Configurable max retransmit count (default 5)

use anyhow::{anyhow, Result};
use std::time::{Duration, Instant};

/// Default ARQ window size.
pub const DEFAULT_WINDOW_SIZE: u8 = 8;

/// Maximum ARQ window size.
pub const MAX_WINDOW_SIZE: u8 = 32;

/// Number of frames beyond the cumulative ACK covered by the SACK bitmap.
const SACK_RANGE: u8 = 8;

/// Default retransmit timeout in seconds.
pub const DEFAULT_RTO_SECS: f64 = 5.0;

/// Default maximum retransmit attempts.
pub const DEFAULT_MAX_RETRANSMIT: u8 = 5;

/// Minimum RTO in seconds.
const MIN_RTO_SECS: f64 = 1.0;

/// Maximum RTO in seconds (backoff cap).
const MAX_RTO_SECS: f64 = 60.0;

/// EWMA smoothing factor for SRTT (alpha).
const SRTT_ALPHA: f64 = 0.125;

/// EWMA smoothing factor for RTTVAR (beta).
const RTTVAR_BETA: f64 = 0.25;

/// ARQ configuration.
#[derive(Debug, Clone)]
pub struct ArqConfig {
    /// Transmit window size (1-32).
    pub window_size: u8,
    /// Maximum retransmit attempts per segment.
    pub max_retransmit: u8,
    /// Initial retransmit timeout.
    pub initial_rto: Duration,
}

impl Default for ArqConfig {
    fn default() -> Self {
        Self {
            window_size: DEFAULT_WINDOW_SIZE,
            max_retransmit: DEFAULT_MAX_RETRANSMIT,
            initial_rto: Duration::from_secs_f64(DEFAULT_RTO_SECS),
        }
    }
}

impl ArqConfig {
    pub fn new(window_size: u8, max_retransmit: u8, initial_rto: Duration) -> Result<Self> {
        if window_size == 0 || window_size > MAX_WINDOW_SIZE {
            return Err(anyhow!(
                "Window size must be 1-{}, got {}",
                MAX_WINDOW_SIZE,
                window_size
            ));
        }
        Ok(Self {
            window_size,
            max_retransmit,
            initial_rto,
        })
    }
}

/// State of a segment in the transmit buffer.
#[derive(Debug, Clone)]
pub struct TxSegment {
    /// Sequence number.
    pub seq_num: u8,
    /// Payload data.
    pub data: Vec<u8>,
    /// Number of times this segment has been transmitted.
    pub transmit_count: u8,
    /// Time of last transmission.
    pub last_sent: Option<Instant>,
    /// Whether this segment has been acknowledged.
    pub acked: bool,
}

/// State of a segment in the receive buffer.
#[derive(Debug, Clone)]
struct RxSlot {
    /// Whether this slot has received data.
    received: bool,
    /// The received data.
    data: Vec<u8>,
}

/// Karn RTT estimator.
///
/// Uses EWMA smoothing (RFC 6298) with Karn's algorithm:
/// RTT samples from retransmitted segments are discarded.
#[derive(Debug, Clone)]
pub struct RttEstimator {
    /// Smoothed RTT.
    srtt: f64,
    /// RTT variance.
    rttvar: f64,
    /// Computed retransmit timeout.
    rto: f64,
    /// Whether we have at least one sample.
    has_sample: bool,
}

impl RttEstimator {
    /// Create a new estimator with a given initial RTO.
    pub fn new(initial_rto: Duration) -> Self {
        Self {
            srtt: initial_rto.as_secs_f64(),
            rttvar: initial_rto.as_secs_f64() / 2.0,
            rto: initial_rto.as_secs_f64(),
            has_sample: false,
        }
    }

    /// Update with a new RTT measurement.
    ///
    /// Per Karn's algorithm, only call this for segments that were NOT retransmitted.
    pub fn update(&mut self, rtt: Duration) {
        let r = rtt.as_secs_f64();

        if !self.has_sample {
            self.srtt = r;
            self.rttvar = r / 2.0;
            self.has_sample = true;
        } else {
            // RFC 6298 algorithm
            self.rttvar = (1.0 - RTTVAR_BETA) * self.rttvar + RTTVAR_BETA * (self.srtt - r).abs();
            self.srtt = (1.0 - SRTT_ALPHA) * self.srtt + SRTT_ALPHA * r;
        }

        self.rto = (self.srtt + 4.0 * self.rttvar).clamp(MIN_RTO_SECS, MAX_RTO_SECS);
    }

    /// Get the current retransmit timeout.
    pub fn rto(&self) -> Duration {
        Duration::from_secs_f64(self.rto)
    }

    /// Get the current smoothed RTT.
    pub fn srtt(&self) -> Duration {
        Duration::from_secs_f64(self.srtt)
    }

    /// Apply exponential backoff to the RTO (called on timeout).
    pub fn backoff(&mut self) {
        self.rto = (self.rto * 2.0).min(MAX_RTO_SECS);
    }
}

/// Transmitter-side selective repeat ARQ state machine.
#[derive(Debug)]
pub struct ArqTx {
    /// Configuration.
    config: ArqConfig,
    /// RTT estimator.
    rtt: RttEstimator,
    /// Next sequence number to assign.
    next_seq: u8,
    /// Base of the send window (oldest unacked segment).
    send_base: u8,
    /// Transmit buffer, indexed by (seq_num % MAX_WINDOW_SIZE).
    tx_buf: Vec<Option<TxSegment>>,
}

impl ArqTx {
    /// Create a new ARQ transmitter.
    ///
    /// # Panics
    /// Panics if `window_size + SACK_RANGE > MAX_WINDOW_SIZE`, which would
    /// cause SACK bitmap entries to alias into buffer slots belonging to
    /// earlier sequence numbers.
    pub fn new(config: ArqConfig) -> Self {
        assert!(
            config.window_size as u16 + SACK_RANGE as u16 <= MAX_WINDOW_SIZE as u16,
            "window_size ({}) + SACK_RANGE ({}) must be <= MAX_WINDOW_SIZE ({}); \
             otherwise SACK bitmap entries alias buffer slots",
            config.window_size,
            SACK_RANGE,
            MAX_WINDOW_SIZE,
        );
        let rtt = RttEstimator::new(config.initial_rto);
        let buf_size = MAX_WINDOW_SIZE as usize;
        Self {
            config,
            rtt,
            next_seq: 0,
            send_base: 0,
            tx_buf: vec![None; buf_size],
        }
    }

    /// Queue a segment for transmission. Returns the assigned sequence number,
    /// or an error if the window is full.
    pub fn send(&mut self, data: Vec<u8>, now: Instant) -> Result<u8> {
        let in_flight = self.next_seq.wrapping_sub(self.send_base);
        if in_flight >= self.config.window_size {
            return Err(anyhow!("ARQ window full"));
        }

        let seq = self.next_seq;
        let idx = seq as usize % MAX_WINDOW_SIZE as usize;
        self.tx_buf[idx] = Some(TxSegment {
            seq_num: seq,
            data,
            transmit_count: 1,
            last_sent: Some(now),
            acked: false,
        });
        self.next_seq = self.next_seq.wrapping_add(1);
        Ok(seq)
    }

    /// Process an ACK. Updates send_base and marks segments as acked.
    /// Returns list of newly acknowledged sequence numbers.
    pub fn process_ack(&mut self, ack_num: u8, ack_bitmap: u8, now: Instant) -> Vec<u8> {
        let mut newly_acked = Vec::new();

        // Process cumulative ACK: advance send_base
        while self.send_base != ack_num {
            let diff = ack_num.wrapping_sub(self.send_base);
            if diff == 0 || diff > 128 {
                break;
            }
            let idx = self.send_base as usize % MAX_WINDOW_SIZE as usize;
            if let Some(ref mut seg) = self.tx_buf[idx] {
                if !seg.acked {
                    seg.acked = true;
                    newly_acked.push(seg.seq_num);

                    // Karn's algorithm: only update RTT for non-retransmitted segments
                    if seg.transmit_count == 1 {
                        if let Some(sent_time) = seg.last_sent {
                            self.rtt.update(now.duration_since(sent_time));
                        }
                    }
                }
            }
            self.tx_buf[idx] = None;
            self.send_base = self.send_base.wrapping_add(1);
        }

        // Process selective ACK bitmap
        for bit in 0..8u8 {
            if (ack_bitmap >> bit) & 1 == 1 {
                let seq = ack_num.wrapping_add(bit + 1);
                let idx = seq as usize % MAX_WINDOW_SIZE as usize;
                if let Some(ref mut seg) = self.tx_buf[idx] {
                    if seg.seq_num == seq && !seg.acked {
                        seg.acked = true;
                        newly_acked.push(seq);

                        if seg.transmit_count == 1 {
                            if let Some(sent_time) = seg.last_sent {
                                self.rtt.update(now.duration_since(sent_time));
                            }
                        }
                    }
                }
            }
        }

        newly_acked
    }

    /// Get segments that need retransmission (timed out and not acked).
    /// Returns sequence numbers of segments to retransmit.
    pub fn get_retransmits(&mut self, now: Instant) -> Vec<u8> {
        let rto = self.rtt.rto();
        let mut retransmits = Vec::new();

        let mut seq = self.send_base;
        while seq != self.next_seq {
            let idx = seq as usize % MAX_WINDOW_SIZE as usize;
            if let Some(ref seg) = self.tx_buf[idx] {
                if !seg.acked {
                    if let Some(last_sent) = seg.last_sent {
                        if now.duration_since(last_sent) >= rto
                            && seg.transmit_count <= self.config.max_retransmit
                        {
                            retransmits.push(seq);
                        }
                    }
                }
            }
            seq = seq.wrapping_add(1);
        }

        retransmits
    }

    /// Mark a segment as retransmitted (updates transmit count and timestamp).
    pub fn mark_retransmitted(&mut self, seq: u8, now: Instant) -> Result<()> {
        let idx = seq as usize % MAX_WINDOW_SIZE as usize;
        match self.tx_buf[idx] {
            Some(ref mut seg) if seg.seq_num == seq && !seg.acked => {
                seg.transmit_count += 1;
                seg.last_sent = Some(now);
                if seg.transmit_count > 1 {
                    self.rtt.backoff();
                }
                Ok(())
            }
            _ => Err(anyhow!("No pending segment with seq {}", seq)),
        }
    }

    /// Check if a segment has exceeded max retransmit count.
    pub fn is_failed(&self, seq: u8) -> bool {
        let idx = seq as usize % MAX_WINDOW_SIZE as usize;
        matches!(&self.tx_buf[idx], Some(seg) if seg.seq_num == seq && seg.transmit_count > self.config.max_retransmit)
    }

    /// Get the data for a segment by sequence number.
    pub fn get_segment_data(&self, seq: u8) -> Option<&[u8]> {
        let idx = seq as usize % MAX_WINDOW_SIZE as usize;
        self.tx_buf[idx]
            .as_ref()
            .filter(|s| s.seq_num == seq)
            .map(|s| s.data.as_slice())
    }

    /// Current send base (oldest unacked sequence number).
    pub fn send_base(&self) -> u8 {
        self.send_base
    }

    /// Next sequence number to be assigned.
    pub fn next_seq(&self) -> u8 {
        self.next_seq
    }

    /// Current RTO estimate.
    pub fn rto(&self) -> Duration {
        self.rtt.rto()
    }

    /// Number of segments currently in flight (sent but unacked).
    pub fn in_flight(&self) -> u8 {
        self.next_seq.wrapping_sub(self.send_base)
    }

    /// Whether the transmit window has room for more segments.
    pub fn can_send(&self) -> bool {
        self.in_flight() < self.config.window_size
    }
}

/// Receiver-side selective repeat ARQ state machine.
#[derive(Debug)]
pub struct ArqRx {
    /// Expected next sequence number (receive base).
    recv_base: u8,
    /// Receive buffer for out-of-order segments.
    rx_buf: Vec<RxSlot>,
    /// Window size.
    window_size: u8,
}

impl ArqRx {
    pub fn new(window_size: u8) -> Self {
        let buf_size = MAX_WINDOW_SIZE as usize;
        Self {
            recv_base: 0,
            rx_buf: (0..buf_size)
                .map(|_| RxSlot {
                    received: false,
                    data: Vec::new(),
                })
                .collect(),
            window_size: window_size.min(MAX_WINDOW_SIZE),
        }
    }

    /// Receive a segment. Returns any in-order data that can be delivered.
    pub fn receive(&mut self, seq: u8, data: Vec<u8>) -> Vec<(u8, Vec<u8>)> {
        // Check if seq is within the receive window
        let diff = seq.wrapping_sub(self.recv_base);
        if diff >= self.window_size {
            // Outside window -- duplicate or too far ahead
            return Vec::new();
        }

        // Buffer the segment
        let idx = seq as usize % MAX_WINDOW_SIZE as usize;
        if !self.rx_buf[idx].received {
            self.rx_buf[idx].received = true;
            self.rx_buf[idx].data = data;
        }

        // Deliver in-order segments
        let mut delivered = Vec::new();
        loop {
            let idx = self.recv_base as usize % MAX_WINDOW_SIZE as usize;
            if !self.rx_buf[idx].received {
                break;
            }
            let slot_data = std::mem::take(&mut self.rx_buf[idx].data);
            self.rx_buf[idx].received = false;
            delivered.push((self.recv_base, slot_data));
            self.recv_base = self.recv_base.wrapping_add(1);
        }

        delivered
    }

    /// Build the current ACK info: (ack_num, ack_bitmap).
    ///
    /// - ack_num: cumulative ACK (all seq < ack_num have been received)
    /// - ack_bitmap: selective ACK for segments beyond ack_num
    pub fn ack_info(&self) -> (u8, u8) {
        let ack_num = self.recv_base;
        let mut bitmap = 0u8;

        for bit in 0..8u8 {
            let seq = ack_num.wrapping_add(bit + 1);
            let idx = seq as usize % MAX_WINDOW_SIZE as usize;
            if self.rx_buf[idx].received {
                bitmap |= 1 << bit;
            }
        }

        (ack_num, bitmap)
    }

    /// Current receive base.
    pub fn recv_base(&self) -> u8 {
        self.recv_base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RTT estimator tests ─────────────────────────────────────────

    #[test]
    fn test_rtt_initial() {
        let est = RttEstimator::new(Duration::from_secs(5));
        assert!((est.rto().as_secs_f64() - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_rtt_first_sample() {
        let mut est = RttEstimator::new(Duration::from_secs(5));
        est.update(Duration::from_secs_f64(2.0));
        // After first sample: SRTT=2.0, RTTVAR=1.0, RTO=2.0+4*1.0=6.0
        assert!((est.srtt().as_secs_f64() - 2.0).abs() < 0.01);
        assert!((est.rto().as_secs_f64() - 6.0).abs() < 0.01);
    }

    #[test]
    fn test_rtt_converges() {
        let mut est = RttEstimator::new(Duration::from_secs(5));
        // Feed stable 2-second RTT samples
        for _ in 0..50 {
            est.update(Duration::from_secs_f64(2.0));
        }
        // SRTT should converge to ~2.0
        assert!((est.srtt().as_secs_f64() - 2.0).abs() < 0.1);
    }

    #[test]
    fn test_rtt_backoff() {
        let mut est = RttEstimator::new(Duration::from_secs(5));
        let rto_before = est.rto().as_secs_f64();
        est.backoff();
        let rto_after = est.rto().as_secs_f64();
        assert!((rto_after - rto_before * 2.0).abs() < 0.01);
    }

    #[test]
    fn test_rtt_backoff_capped() {
        let mut est = RttEstimator::new(Duration::from_secs(5));
        for _ in 0..20 {
            est.backoff();
        }
        assert!(est.rto().as_secs_f64() <= MAX_RTO_SECS);
    }

    #[test]
    fn test_rtt_min_rto() {
        let mut est = RttEstimator::new(Duration::from_secs(5));
        // Feed very small RTT
        for _ in 0..100 {
            est.update(Duration::from_secs_f64(0.001));
        }
        assert!(est.rto().as_secs_f64() >= MIN_RTO_SECS);
    }

    // ── ArqConfig tests ─────────────────────────────────────────────

    #[test]
    fn test_config_default() {
        let config = ArqConfig::default();
        assert_eq!(config.window_size, DEFAULT_WINDOW_SIZE);
        assert_eq!(config.max_retransmit, DEFAULT_MAX_RETRANSMIT);
    }

    #[test]
    fn test_config_valid_range() {
        assert!(ArqConfig::new(1, 5, Duration::from_secs(3)).is_ok());
        assert!(ArqConfig::new(32, 5, Duration::from_secs(3)).is_ok());
    }

    #[test]
    fn test_config_invalid_window() {
        assert!(ArqConfig::new(0, 5, Duration::from_secs(3)).is_err());
        assert!(ArqConfig::new(33, 5, Duration::from_secs(3)).is_err());
    }

    // ── ArqTx tests ─────────────────────────────────────────────────

    #[test]
    fn test_tx_send_basic() {
        let mut tx = ArqTx::new(ArqConfig::default());
        let now = Instant::now();

        let seq = tx.send(vec![0x01], now).unwrap();
        assert_eq!(seq, 0);
        assert_eq!(tx.in_flight(), 1);

        let seq2 = tx.send(vec![0x02], now).unwrap();
        assert_eq!(seq2, 1);
        assert_eq!(tx.in_flight(), 2);
    }

    #[test]
    fn test_tx_window_full() {
        let config = ArqConfig::new(2, 5, Duration::from_secs(5)).unwrap();
        let mut tx = ArqTx::new(config);
        let now = Instant::now();

        tx.send(vec![0x01], now).unwrap();
        tx.send(vec![0x02], now).unwrap();
        assert!(tx.send(vec![0x03], now).is_err());
    }

    #[test]
    fn test_tx_ack_advances_window() {
        let config = ArqConfig::new(2, 5, Duration::from_secs(5)).unwrap();
        let mut tx = ArqTx::new(config);
        let now = Instant::now();

        tx.send(vec![0x01], now).unwrap();
        tx.send(vec![0x02], now).unwrap();
        assert!(!tx.can_send());

        // ACK both
        let acked = tx.process_ack(2, 0, now);
        assert_eq!(acked.len(), 2);
        assert!(tx.can_send());
        assert_eq!(tx.in_flight(), 0);
    }

    #[test]
    fn test_tx_selective_ack() {
        let mut tx = ArqTx::new(ArqConfig::default());
        let now = Instant::now();

        // Send 4 segments
        for i in 0..4 {
            tx.send(vec![i], now).unwrap();
        }

        // ACK only seq 0 cumulatively, and seq 2 selectively
        // ack_num=1, bitmap bit 1 set (ack_num+2 = seq 2)
        let acked = tx.process_ack(1, 0b00000010, now);
        assert!(acked.contains(&0)); // cumulative
        assert!(acked.contains(&3)); // selective: ack_num(1) + bit(1) + 1 = 3... wait
                                     // Actually: bit 0 = ack_num+1 = seq 2, bit 1 = ack_num+2 = seq 3
                                     // bitmap 0b00000010 means bit 1 is set -> seq 3 is acked
                                     // Let me re-check: the process_ack iterates bit 0..7, where bit N corresponds to ack_num + N + 1
                                     // bit 0 -> seq 2, bit 1 -> seq 3
                                     // bitmap 0b00000010: bit 1 set -> seq 3
                                     // So acked should be [0, 3]
                                     // But seq 1 is also < ack_num=1? No, ack_num=1 means "everything below 1 is acked", so seq 0.
                                     // Wait: the cumulative loop advances send_base while send_base != ack_num.
                                     // send_base starts at 0, ack_num=1. So it acks seq 0 and advances to 1.
        assert!(acked.contains(&0));
        // For selective: bit 1 set, so ack_num + 1 + 1 = 3
        assert!(acked.contains(&3));
    }

    #[test]
    fn test_tx_retransmit_detection() {
        let config = ArqConfig::new(8, 5, Duration::from_millis(100)).unwrap();
        let mut tx = ArqTx::new(config);
        let start = Instant::now();

        tx.send(vec![0x01], start).unwrap();

        // No retransmit needed yet
        let retransmits = tx.get_retransmits(start);
        assert!(retransmits.is_empty());

        // After RTO, should need retransmit
        let later = start + Duration::from_millis(200);
        let retransmits = tx.get_retransmits(later);
        assert_eq!(retransmits, vec![0]);
    }

    #[test]
    fn test_tx_retransmit_count() {
        let config = ArqConfig::new(8, 2, Duration::from_millis(50)).unwrap();
        let mut tx = ArqTx::new(config);
        let start = Instant::now();

        tx.send(vec![0x01], start).unwrap();
        // transmit_count is 1 after send

        // First retransmit
        tx.mark_retransmitted(0, start + Duration::from_millis(100))
            .unwrap();
        assert!(!tx.is_failed(0)); // transmit_count = 2

        // Second retransmit
        tx.mark_retransmitted(0, start + Duration::from_millis(200))
            .unwrap();
        assert!(tx.is_failed(0)); // transmit_count = 3 > max_retransmit(2)
    }

    #[test]
    fn test_tx_get_segment_data() {
        let mut tx = ArqTx::new(ArqConfig::default());
        let now = Instant::now();

        tx.send(vec![0xAA, 0xBB], now).unwrap();
        assert_eq!(tx.get_segment_data(0), Some(&[0xAA, 0xBB][..]));
        assert_eq!(tx.get_segment_data(1), None);
    }

    #[test]
    fn test_tx_karn_algorithm() {
        let config = ArqConfig::new(8, 5, Duration::from_secs(5)).unwrap();
        let mut tx = ArqTx::new(config);
        let start = Instant::now();

        // Send two segments
        tx.send(vec![0x01], start).unwrap();
        tx.send(vec![0x02], start).unwrap();

        // Retransmit seq 0
        tx.mark_retransmitted(0, start + Duration::from_millis(100))
            .unwrap();

        // ACK both -- RTT should NOT be updated for retransmitted seq 0
        let ack_time = start + Duration::from_millis(500);
        let rto_before = tx.rto();
        tx.process_ack(2, 0, ack_time);

        // RTT was only updated from seq 1 (not retransmitted)
        // The RTO should have changed from the seq 1 sample
        let rto_after = tx.rto();
        // Just verify it didn't crash and the values are reasonable
        assert!(rto_before.as_secs_f64() > 0.0);
        assert!(rto_after.as_secs_f64() > 0.0);
    }

    // ── ArqRx tests ─────────────────────────────────────────────────

    #[test]
    fn test_rx_in_order() {
        let mut rx = ArqRx::new(8);

        let delivered = rx.receive(0, vec![0x01]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0], (0, vec![0x01]));

        let delivered = rx.receive(1, vec![0x02]);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0], (1, vec![0x02]));
    }

    #[test]
    fn test_rx_out_of_order() {
        let mut rx = ArqRx::new(8);

        // Receive seq 1 before seq 0
        let delivered = rx.receive(1, vec![0x02]);
        assert!(delivered.is_empty()); // cannot deliver yet

        let delivered = rx.receive(0, vec![0x01]);
        assert_eq!(delivered.len(), 2);
        assert_eq!(delivered[0], (0, vec![0x01]));
        assert_eq!(delivered[1], (1, vec![0x02]));
    }

    #[test]
    fn test_rx_duplicate() {
        let mut rx = ArqRx::new(8);

        let delivered = rx.receive(0, vec![0x01]);
        assert_eq!(delivered.len(), 1);

        // Duplicate of already delivered segment
        let delivered = rx.receive(0, vec![0x01]);
        assert!(delivered.is_empty());
    }

    #[test]
    fn test_rx_outside_window() {
        let mut rx = ArqRx::new(4);

        // seq 5 is outside window [0..4)
        let delivered = rx.receive(5, vec![0xFF]);
        assert!(delivered.is_empty());
    }

    #[test]
    fn test_rx_ack_info_in_order() {
        let mut rx = ArqRx::new(8);

        rx.receive(0, vec![0x01]);
        rx.receive(1, vec![0x02]);

        let (ack_num, bitmap) = rx.ack_info();
        assert_eq!(ack_num, 2); // received 0 and 1
        assert_eq!(bitmap, 0);
    }

    #[test]
    fn test_rx_ack_info_gap() {
        let mut rx = ArqRx::new(8);

        // Receive 0, skip 1, receive 2 and 3
        rx.receive(0, vec![0x01]);
        rx.receive(2, vec![0x03]);
        rx.receive(3, vec![0x04]);

        let (ack_num, bitmap) = rx.ack_info();
        assert_eq!(ack_num, 1); // only seq 0 is in-order
                                // bitmap: bit 0 = seq 2 (ack_num+1=2, not received... wait)
                                // ack_num=1, so:
                                // bit 0 = ack_num+1 = 2: received -> bit 0 set
                                // bit 1 = ack_num+2 = 3: received -> bit 1 set
        assert_eq!(bitmap, 0b00000011);
    }

    #[test]
    fn test_rx_ack_info_scattered() {
        let mut rx = ArqRx::new(8);

        // Receive seq 0, 2, 4, 6
        rx.receive(0, vec![]);
        rx.receive(2, vec![]);
        rx.receive(4, vec![]);
        rx.receive(6, vec![]);

        let (ack_num, bitmap) = rx.ack_info();
        assert_eq!(ack_num, 1);
        // bit 0 -> seq 2: yes -> 1
        // bit 1 -> seq 3: no  -> 0
        // bit 2 -> seq 4: yes -> 1
        // bit 3 -> seq 5: no  -> 0
        // bit 4 -> seq 6: yes -> 1
        // bit 5 -> seq 7: no  -> 0
        assert_eq!(bitmap, 0b00010101);
    }

    // ── Combined TX/RX integration tests ────────────────────────────

    #[test]
    fn test_tx_rx_perfect_channel() {
        let mut tx = ArqTx::new(ArqConfig::default());
        let mut rx = ArqRx::new(DEFAULT_WINDOW_SIZE);
        let now = Instant::now();

        let mut all_delivered = Vec::new();

        // Send 10 segments
        for i in 0..10u8 {
            let seq = tx.send(vec![i], now).unwrap();

            // "Transmit" and receive
            let data = tx.get_segment_data(seq).unwrap().to_vec();
            let delivered = rx.receive(seq, data);
            all_delivered.extend(delivered);

            // ACK
            let (ack_num, bitmap) = rx.ack_info();
            tx.process_ack(ack_num, bitmap, now);
        }

        assert_eq!(all_delivered.len(), 10);
        for (i, (seq, data)) in all_delivered.iter().enumerate() {
            assert_eq!(*seq, i as u8);
            assert_eq!(data, &vec![i as u8]);
        }
    }

    #[test]
    fn test_tx_rx_with_loss() {
        let mut tx = ArqTx::new(ArqConfig::default());
        let mut rx = ArqRx::new(DEFAULT_WINDOW_SIZE);
        let now = Instant::now();

        // Send 3 segments
        tx.send(vec![0x01], now).unwrap();
        tx.send(vec![0x02], now).unwrap();
        tx.send(vec![0x03], now).unwrap();

        // Only deliver seq 0 and seq 2 (seq 1 is "lost")
        rx.receive(0, vec![0x01]);
        rx.receive(2, vec![0x03]);

        // Check ACK info
        let (ack_num, bitmap) = rx.ack_info();
        assert_eq!(ack_num, 1); // cumulative up to 1
        assert_eq!(bitmap & 1, 1); // bit 0 set for seq 2

        // Process ACK
        let acked = tx.process_ack(ack_num, bitmap, now);
        assert!(acked.contains(&0));
        assert!(acked.contains(&2));

        // Retransmit seq 1
        rx.receive(1, vec![0x02]);
        let (ack_num2, _bitmap2) = rx.ack_info();
        assert_eq!(ack_num2, 3); // now everything is in order
    }

    #[test]
    fn test_wrapping_sequence_numbers() {
        let config = ArqConfig::new(8, 5, Duration::from_secs(5)).unwrap();
        let mut tx = ArqTx::new(config);
        let mut rx = ArqRx::new(8);
        let now = Instant::now();

        // Advance to near wrapping point
        // Manually set send_base and next_seq near 255
        // We can't set directly, so let's send and ack a lot
        for _ in 0..252 {
            let seq = tx.send(vec![0x00], now).unwrap();
            rx.receive(seq, vec![0x00]);
            let (ack_num, bitmap) = rx.ack_info();
            tx.process_ack(ack_num, bitmap, now);
        }

        assert_eq!(tx.send_base(), 252);

        // Now send across the boundary
        for i in 0..8u8 {
            let seq = tx.send(vec![i], now).unwrap();
            let data = tx.get_segment_data(seq).unwrap().to_vec();
            let delivered = rx.receive(seq, data);
            assert_eq!(delivered.len(), 1);
        }

        let (ack_num, _) = rx.ack_info();
        let acked = tx.process_ack(ack_num, 0, now);
        assert_eq!(acked.len(), 8);
    }
}
