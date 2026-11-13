//! Shared waveform policy for session adapters. Timers belong to the adapter.
use std::time::Duration;

/// Physical mode of a validated or transmitted session frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkPhyMode {
    /// Separate eight-second low-rate waveform.
    Floor,
    /// Numbered OFDM waveform.
    Ofdm {
        /// Speed level.
        level: u8,
    },
}

/// Waveform selection state. Call only at transmission boundaries.
#[derive(Debug, Default)]
pub struct FloorController {
    unanswered: u8,
    floor: bool,
    acknowledged: u8,
    probe: Option<u8>,
}

impl FloorController {
    /// Expire one complete unanswered response window.
    pub fn timeout(&mut self, enabled: bool) {
        self.unanswered = self.unanswered.saturating_add(1);
        if enabled && self.unanswered >= 2 {
            self.floor = true;
        }
        if self.probe.take().is_some() {
            self.floor = true;
            self.acknowledged = 0;
        }
    }

    /// Select the waveform for the next transmission.
    pub fn mode(&self, level: u8) -> LinkPhyMode {
        if self.floor && self.probe.is_none() {
            LinkPhyMode::Floor
        } else {
            LinkPhyMode::Ofdm {
                level: if self.probe.is_some() { 1 } else { level },
            }
        }
    }

    /// Follow a validated peer transmission, except during our outstanding probe.
    pub fn received(&mut self, mode: LinkPhyMode) {
        if self.probe.is_none() {
            self.floor = mode == LinkPhyMode::Floor;
        }
    }

    /// Record a newly acknowledged exchange. The adapter must reject stale ACKs.
    pub fn acknowledged(&mut self, seq: u8, mode: LinkPhyMode) {
        self.unanswered = 0;
        if self.probe == Some(seq) && matches!(mode, LinkPhyMode::Ofdm { .. }) {
            self.floor = false;
            self.probe = None;
            self.acknowledged = 0;
        } else if self.floor && self.probe.is_none() && mode == LinkPhyMode::Floor {
            self.acknowledged = self.acknowledged.saturating_add(1);
        }
    }

    /// Bind the single recovery probe to its next outstanding sequence.
    pub fn prepare_data(&mut self, seq: u8) {
        if self.floor && self.acknowledged >= 5 && self.probe.is_none() {
            self.probe = Some(seq);
        }
    }

    /// Request and response airtime plus two turnarounds, measured from key-up.
    pub fn response_window(mode: LinkPhyMode, ofdm: Duration, turnaround: Duration) -> Duration {
        let airtime = if mode == LinkPhyMode::Floor {
            Duration::from_secs(8)
        } else {
            ofdm
        };
        airtime * 2
            + turnaround * 2
            + if mode == LinkPhyMode::Floor {
                Duration::from_secs(2)
            } else {
                Duration::ZERO
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floor_two_window_fallback() {
        let mut c = FloorController::default();
        c.timeout(true);
        assert_eq!(c.mode(2), LinkPhyMode::Ofdm { level: 2 });
        c.timeout(true);
        assert_eq!(c.mode(2), LinkPhyMode::Floor);
        assert_eq!(
            FloorController::response_window(
                LinkPhyMode::Floor,
                Duration::ZERO,
                Duration::from_millis(500)
            ),
            Duration::from_secs(19)
        );
        let mut legacy = FloorController::default();
        for _ in 0..10 {
            legacy.timeout(false);
        }
        assert_eq!(legacy.mode(2), LinkPhyMode::Ofdm { level: 2 });
    }
    #[test]
    fn floor_five_ack_probe() {
        let mut c = FloorController::default();
        c.received(LinkPhyMode::Floor);
        for seq in 0..5 {
            c.prepare_data(seq);
            assert_eq!(c.mode(2), LinkPhyMode::Floor);
            c.acknowledged(seq, LinkPhyMode::Floor);
        }
        c.prepare_data(5);
        assert_eq!(c.mode(2), LinkPhyMode::Ofdm { level: 1 });
        c.acknowledged(4, LinkPhyMode::Ofdm { level: 1 });
        c.timeout(true);
        assert_eq!(c.mode(2), LinkPhyMode::Floor);
        for seq in 5..10 {
            c.acknowledged(seq, LinkPhyMode::Floor);
        }
        c.prepare_data(10);
        c.acknowledged(10, LinkPhyMode::Ofdm { level: 1 });
        assert_eq!(c.mode(2), LinkPhyMode::Ofdm { level: 2 });
    }
}
