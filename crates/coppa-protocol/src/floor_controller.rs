//! Shared waveform policy for session adapters. Timers belong to the adapter.
//!
//! Both session adapters, the library `Link` (coppa-engine) and coppad's
//! event loop (coppa-daemon), own a [`FloorController`] and call the free
//! policy functions below, so the floor gate, segment limit and timers are
//! computed by one piece of code (COP-195, COP-202).
use std::time::Duration;

use crate::mac::{MacFrameType, MacPdu};
use crate::modem::floor::MAX_PAYLOAD as FLOOR_MAX_PAYLOAD;
use crate::session::{LinkCapabilities, Session, SessionState, HANDSHAKE_TIMEOUT_SECS};
use crate::transport::TRANSPORT_HEADER_SIZE;

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

/// Whether a decoded floor PDU may reach the session handlers. Used by both
/// the library `Link` and coppad's event loop.
///
/// `session` is the session the adapter matched to the PDU's source,
/// destination and SSID (`None` when there is none). With the local floor
/// disabled nothing is admitted. A CONNECT_REQ or CONNECT_ACK is admitted
/// when its capabilities carry `robust_floor` (the floor bootstrap); a
/// CONNECT_CFM is admitted for an `Accepting` session whose local and remote
/// capabilities both carry the bit; any other PDU only once the session
/// negotiated the floor.
pub fn admits_floor_pdu(
    frame_type: MacFrameType,
    payload: &[u8],
    session: Option<&Session>,
    local_enabled: bool,
) -> bool {
    if !local_enabled {
        return false;
    }
    let bootstrap = matches!(
        frame_type,
        MacFrameType::ConnectReq | MacFrameType::ConnectAck
    ) && LinkCapabilities::from_bytes(payload).is_ok_and(|c| c.robust_floor);
    let confirming = frame_type == MacFrameType::ConnectCfm
        && session.is_some_and(|s| {
            s.state == SessionState::Accepting
                && s.local_caps.robust_floor
                && s.remote_caps.as_ref().is_some_and(|c| c.robust_floor)
        });
    let negotiated = session
        .and_then(|s| s.negotiated.as_ref())
        .is_some_and(|c| c.robust_floor);
    bootstrap || confirming || negotiated
}

/// Floor-capable capabilities whose payload limit cannot carry a single
/// session data byte on the floor. Both adapters drop such handshakes when
/// their own floor is enabled.
pub fn floor_caps_unusable(caps: &LinkCapabilities) -> bool {
    caps.robust_floor
        && usize::from(caps.max_payload_size) <= MacPdu::HEADER_SIZE + TRANSPORT_HEADER_SIZE
}

/// Largest session data segment, in bytes, whose MAC PDU fits one floor
/// frame under the negotiated payload limit. Used by both adapters.
pub fn floor_segment_limit(negotiated_max_payload: u16) -> usize {
    FLOOR_MAX_PAYLOAD
        .min(usize::from(negotiated_max_payload))
        .saturating_sub(MacPdu::HEADER_SIZE + TRANSPORT_HEADER_SIZE)
}

/// Minimum retransmission timeout for a floor exchange: the floor response
/// window plus one second of delivery margin. Used by both adapters.
pub fn floor_rto_floor(turnaround: Duration) -> Duration {
    FloorController::response_window(LinkPhyMode::Floor, Duration::ZERO, turnaround)
        + Duration::from_secs(1)
}

/// Keepalive interval for a floor-negotiated session: at least 60 s, so a
/// keepalive never keys across a failed probe's recovery window, and at most
/// a third of the session timeout. Used by both adapters.
pub fn floor_keepalive_interval(keepalive: Duration, session_timeout: Duration) -> Duration {
    keepalive
        .max(Duration::from_secs(60))
        .min(session_timeout / 3)
}

/// Idle deadline for a calling session that still has a connect request
/// to repeat: the flat handshake timeout, extended past the retry interval
/// and its turnaround hold. Used by both adapters.
pub fn calling_handshake_timeout(retry_interval: Duration, turnaround: Duration) -> Duration {
    Duration::from_secs(HANDSHAKE_TIMEOUT_SECS)
        .max(retry_interval + turnaround + Duration::from_secs(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::Callsign;
    use std::time::Instant;

    fn caps(robust_floor: bool) -> LinkCapabilities {
        LinkCapabilities {
            robust_floor,
            ..LinkCapabilities::default()
        }
    }

    fn session(
        state: SessionState,
        local: bool,
        remote: bool,
        negotiated: Option<bool>,
    ) -> Session {
        let mut s = Session::new(
            0,
            Callsign::new("VK3ABC").unwrap(),
            Callsign::new("W1AW").unwrap(),
            0,
            caps(local),
            Instant::now(),
        );
        s.state = state;
        s.remote_caps = Some(caps(remote));
        s.negotiated = negotiated.map(caps);
        s
    }

    #[test]
    fn floor_pdu_gate_admits_bootstrap_confirm_and_negotiated() {
        let with = caps(true).to_bytes();
        let without = caps(false).to_bytes();
        for t in [MacFrameType::ConnectReq, MacFrameType::ConnectAck] {
            assert!(admits_floor_pdu(t, &with, None, true));
            assert!(!admits_floor_pdu(t, &without, None, true));
            assert!(!admits_floor_pdu(t, &with, None, false));
        }
        let accepting = session(SessionState::Accepting, true, true, None);
        assert!(admits_floor_pdu(
            MacFrameType::ConnectCfm,
            &with,
            Some(&accepting),
            true
        ));
        assert!(!admits_floor_pdu(
            MacFrameType::ConnectCfm,
            &with,
            None,
            true
        ));
        let one_sided = session(SessionState::Accepting, true, false, None);
        assert!(!admits_floor_pdu(
            MacFrameType::ConnectCfm,
            &with,
            Some(&one_sided),
            true
        ));
        let connecting = session(SessionState::Connecting, true, true, None);
        assert!(!admits_floor_pdu(
            MacFrameType::ConnectCfm,
            &with,
            Some(&connecting),
            true
        ));
        let negotiated = session(SessionState::Established, true, true, Some(true));
        let legacy = session(SessionState::Established, true, false, Some(false));
        for t in [
            MacFrameType::Data,
            MacFrameType::Ack,
            MacFrameType::Keepalive,
            MacFrameType::Disconnect,
        ] {
            assert!(admits_floor_pdu(t, &[], Some(&negotiated), true));
            assert!(!admits_floor_pdu(t, &[], Some(&legacy), true));
            assert!(!admits_floor_pdu(t, &[], None, true));
            assert!(!admits_floor_pdu(t, &[], Some(&negotiated), false));
        }
    }

    #[test]
    fn floor_segment_limit_and_unusable_caps() {
        assert_eq!(floor_segment_limit(200), 19);
        assert_eq!(floor_segment_limit(30), 9);
        assert_eq!(floor_segment_limit(21), 0);
        let small = LinkCapabilities {
            max_payload_size: 21,
            ..caps(true)
        };
        assert!(floor_caps_unusable(&small));
        assert!(!floor_caps_unusable(&LinkCapabilities {
            max_payload_size: 22,
            ..caps(true)
        }));
        assert!(!floor_caps_unusable(&LinkCapabilities {
            robust_floor: false,
            ..small
        }));
    }

    #[test]
    fn floor_session_timers() {
        let ms = Duration::from_millis;
        let s = Duration::from_secs;
        assert_eq!(floor_rto_floor(ms(500)), s(20));
        assert_eq!(floor_keepalive_interval(s(30), s(120)), s(40));
        assert_eq!(floor_keepalive_interval(s(30), s(600)), s(60));
        assert_eq!(calling_handshake_timeout(s(20), ms(500)), s(30));
        assert_eq!(calling_handshake_timeout(s(31), ms(500)), ms(32_500));
    }
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
