//! Session management for Coppa protocol.
//!
//! Implements the 3-way handshake for session establishment:
//!
//! ```text
//! Initiator                           Responder
//!     |--- CONNECT_REQ (capabilities) --->|
//!     |<-- CONNECT_ACK (capabilities) ----|
//!     |--- CONNECT_CFM (negotiated)   --->|
//!     |          ESTABLISHED              |
//! ```
//!
//! Sessions are identified by a 4-bit session ID (0-15) and maintain state
//! for the ARQ layer, keepalive timers, and negotiated link parameters.

use anyhow::{anyhow, Result};
use std::time::{Duration, Instant};

use crate::mac::{Callsign, MacFrameType, MacPdu};

/// Session states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No session.
    Idle,
    /// CONNECT_REQ sent, waiting for CONNECT_ACK.
    Connecting,
    /// CONNECT_ACK sent, waiting for CONNECT_CFM.
    Accepting,
    /// Three-way handshake complete.
    Established,
    /// Disconnecting (DISCONNECT sent or received).
    Disconnecting,
}

/// Link capabilities advertised during connection setup.
///
/// Each peer advertises its capabilities in CONNECT_REQ/CONNECT_ACK,
/// and the negotiated result is confirmed in CONNECT_CFM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCapabilities {
    /// Maximum frame payload size in bytes.
    pub max_payload_size: u16,
    /// Supported FEC code rate numerator (e.g., 1 for rate 1/2).
    pub fec_rate_num: u8,
    /// Supported FEC code rate denominator (e.g., 2 for rate 1/2).
    pub fec_rate_den: u8,
    /// ARQ window size.
    pub arq_window: u8,
    /// Whether compression is supported.
    pub compression: bool,
    /// Modulation/coding scheme index.
    pub mcs_index: u8,
}

impl Default for LinkCapabilities {
    fn default() -> Self {
        Self {
            max_payload_size: 200,
            fec_rate_num: 1,
            fec_rate_den: 2,
            arq_window: 8,
            compression: false,
            mcs_index: 0,
        }
    }
}

impl LinkCapabilities {
    /// Serialize capabilities to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7);
        out.push((self.max_payload_size >> 8) as u8);
        out.push((self.max_payload_size & 0xFF) as u8);
        out.push(self.fec_rate_num);
        out.push(self.fec_rate_den);
        out.push(self.arq_window);
        let flags = if self.compression { 0x01 } else { 0x00 };
        out.push(flags);
        out.push(self.mcs_index);
        out
    }

    /// Deserialize capabilities from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 7 {
            return Err(anyhow!(
                "Capabilities too short: {} bytes (need 7)",
                bytes.len()
            ));
        }
        Ok(Self {
            max_payload_size: ((bytes[0] as u16) << 8) | bytes[1] as u16,
            fec_rate_num: bytes[2],
            fec_rate_den: bytes[3],
            arq_window: bytes[4],
            compression: bytes[5] & 0x01 != 0,
            mcs_index: bytes[6],
        })
    }

    /// Negotiate capabilities: take the minimum/intersection of both sides.
    pub fn negotiate(&self, peer: &LinkCapabilities) -> LinkCapabilities {
        LinkCapabilities {
            max_payload_size: self.max_payload_size.min(peer.max_payload_size),
            fec_rate_num: if self.fec_rate() <= peer.fec_rate() {
                self.fec_rate_num
            } else {
                peer.fec_rate_num
            },
            fec_rate_den: if self.fec_rate() <= peer.fec_rate() {
                self.fec_rate_den
            } else {
                peer.fec_rate_den
            },
            arq_window: self.arq_window.min(peer.arq_window),
            compression: self.compression && peer.compression,
            mcs_index: self.mcs_index.min(peer.mcs_index),
        }
    }

    pub(crate) fn fec_rate(&self) -> f32 {
        if self.fec_rate_den == 0 {
            return 0.0;
        }
        self.fec_rate_num as f32 / self.fec_rate_den as f32
    }
}

/// A session between two stations.
#[derive(Debug)]
pub struct Session {
    /// Session ID (0-15).
    pub id: u8,
    /// Current state.
    pub state: SessionState,
    /// Local callsign.
    pub local: Callsign,
    /// Remote callsign.
    pub remote: Callsign,
    /// SSID for this session.
    pub ssid: u8,
    /// Our advertised capabilities.
    pub local_caps: LinkCapabilities,
    /// Peer's advertised capabilities.
    pub remote_caps: Option<LinkCapabilities>,
    /// Negotiated capabilities (set after CONNECT_CFM).
    pub negotiated: Option<LinkCapabilities>,
    /// Time the session was created.
    pub created_at: Instant,
    /// Time of last activity.
    pub last_activity: Instant,
    /// Keepalive interval.
    pub keepalive_interval: Duration,
    /// Session timeout (disconnect if no activity for this long).
    pub session_timeout: Duration,
    /// Number of CONNECT_CFM retransmissions sent (initiator side).
    pub cfm_retries: u8,
    /// Cached CONNECT_CFM PDU for retransmission (initiator side).
    pub last_cfm: Option<MacPdu>,
}

/// Default keepalive interval.
pub const DEFAULT_KEEPALIVE_SECS: u64 = 30;

/// Default session timeout.
pub const DEFAULT_SESSION_TIMEOUT_SECS: u64 = 120;

/// Default disconnect timeout (shorter than session timeout).
pub const DEFAULT_DISCONNECT_TIMEOUT_SECS: u64 = 10;

/// Maximum connect retry count.
pub const MAX_CONNECT_RETRIES: u8 = 3;

/// Handshake timeout in seconds (shorter than full session timeout).
/// Applies to Connecting and Accepting states where the handshake should
/// complete quickly or be abandoned.
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 30;

/// Maximum number of CONNECT_CFM retransmissions before giving up.
pub const MAX_CFM_RETRIES: u8 = 3;

impl Session {
    /// Create a new session in Idle state.
    pub fn new(
        id: u8,
        local: Callsign,
        remote: Callsign,
        ssid: u8,
        local_caps: LinkCapabilities,
    ) -> Self {
        let now = Instant::now();
        Self {
            id: id & 0x0F,
            state: SessionState::Idle,
            local,
            remote,
            ssid,
            local_caps,
            remote_caps: None,
            negotiated: None,
            created_at: now,
            last_activity: now,
            keepalive_interval: Duration::from_secs(DEFAULT_KEEPALIVE_SECS),
            session_timeout: Duration::from_secs(DEFAULT_SESSION_TIMEOUT_SECS),
            cfm_retries: 0,
            last_cfm: None,
        }
    }

    // ── Initiator side ──────────────────────────────────────────────

    /// Initiate a connection: create a CONNECT_REQ PDU.
    pub fn initiate(&mut self) -> Result<MacPdu> {
        if self.state != SessionState::Idle {
            return Err(anyhow!(
                "Cannot initiate: session in state {:?}",
                self.state
            ));
        }

        self.state = SessionState::Connecting;
        self.last_activity = Instant::now();

        Ok(MacPdu::new(
            MacFrameType::ConnectReq,
            self.remote.clone(),
            self.local.clone(),
            self.ssid,
            self.local_caps.to_bytes(),
        ))
    }

    /// Handle CONNECT_ACK response (initiator side).
    /// Returns a CONNECT_CFM PDU with negotiated capabilities.
    pub fn handle_connect_ack(&mut self, remote_caps_bytes: &[u8]) -> Result<MacPdu> {
        if self.state != SessionState::Connecting {
            return Err(anyhow!("Unexpected CONNECT_ACK in state {:?}", self.state));
        }

        let remote_caps = LinkCapabilities::from_bytes(remote_caps_bytes)?;
        let negotiated = self.local_caps.negotiate(&remote_caps);

        self.remote_caps = Some(remote_caps);
        self.negotiated = Some(negotiated.clone());
        self.state = SessionState::Established;
        self.last_activity = Instant::now();

        let cfm_pdu = MacPdu::new(
            MacFrameType::ConnectCfm,
            self.remote.clone(),
            self.local.clone(),
            self.ssid,
            negotiated.to_bytes(),
        );
        self.last_cfm = Some(cfm_pdu.clone());
        self.cfm_retries = 0;

        Ok(cfm_pdu)
    }

    // ── Responder side ──────────────────────────────────────────────

    /// Handle CONNECT_REQ (responder side).
    /// Returns a CONNECT_ACK PDU with our capabilities.
    pub fn handle_connect_req(&mut self, remote_caps_bytes: &[u8]) -> Result<MacPdu> {
        if self.state != SessionState::Idle {
            return Err(anyhow!("Unexpected CONNECT_REQ in state {:?}", self.state));
        }

        let remote_caps = LinkCapabilities::from_bytes(remote_caps_bytes)?;
        self.remote_caps = Some(remote_caps);
        self.state = SessionState::Accepting;
        self.last_activity = Instant::now();

        Ok(MacPdu::new(
            MacFrameType::ConnectAck,
            self.remote.clone(),
            self.local.clone(),
            self.ssid,
            self.local_caps.to_bytes(),
        ))
    }

    /// Handle CONNECT_CFM (responder side).
    /// Validates that negotiated capabilities are consistent with both sides'
    /// advertised capabilities, then transitions to Established.
    pub fn handle_connect_cfm(&mut self, negotiated_bytes: &[u8]) -> Result<()> {
        if self.state != SessionState::Accepting {
            return Err(anyhow!("Unexpected CONNECT_CFM in state {:?}", self.state));
        }

        let negotiated = LinkCapabilities::from_bytes(negotiated_bytes)?;

        // Validate that negotiated caps don't exceed either side's advertised caps
        if let Some(ref remote) = self.remote_caps {
            let expected = self.local_caps.negotiate(remote);
            if negotiated.max_payload_size > expected.max_payload_size
                || negotiated.arq_window > expected.arq_window
                || negotiated.mcs_index > expected.mcs_index
            {
                return Err(anyhow!(
                    "Negotiated capabilities exceed intersection: payload={} (max {}), window={} (max {})",
                    negotiated.max_payload_size,
                    expected.max_payload_size,
                    negotiated.arq_window,
                    expected.arq_window,
                ));
            }
            // Validate FEC rate does not exceed negotiated rate
            if negotiated.fec_rate() > expected.fec_rate() + f32::EPSILON {
                return Err(anyhow!(
                    "Negotiated FEC rate {}/{} exceeds intersection {}/{}",
                    negotiated.fec_rate_num,
                    negotiated.fec_rate_den,
                    expected.fec_rate_num,
                    expected.fec_rate_den,
                ));
            }
            // Compression must not be enabled unless both sides support it
            if negotiated.compression && !expected.compression {
                return Err(anyhow!(
                    "Negotiated compression enabled but not supported by both peers"
                ));
            }
        }

        self.negotiated = Some(negotiated);
        self.state = SessionState::Established;
        self.last_activity = Instant::now();

        Ok(())
    }

    // ── Common operations ───────────────────────────────────────────

    /// Create a DISCONNECT PDU.
    pub fn disconnect(&mut self) -> Result<MacPdu> {
        if self.state == SessionState::Idle {
            return Err(anyhow!("Cannot disconnect: session is idle"));
        }

        self.state = SessionState::Disconnecting;
        self.last_activity = Instant::now();

        Ok(MacPdu::new(
            MacFrameType::Disconnect,
            self.remote.clone(),
            self.local.clone(),
            self.ssid,
            vec![],
        ))
    }

    /// Handle a received DISCONNECT.
    pub fn handle_disconnect(&mut self) {
        self.state = SessionState::Idle;
        self.negotiated = None;
        self.remote_caps = None;
    }

    /// Create a KEEPALIVE PDU.
    pub fn keepalive(&mut self) -> Result<MacPdu> {
        if self.state != SessionState::Established {
            return Err(anyhow!("Cannot send keepalive: session not established"));
        }

        self.last_activity = Instant::now();

        Ok(MacPdu::new(
            MacFrameType::Keepalive,
            self.remote.clone(),
            self.local.clone(),
            self.ssid,
            vec![],
        ))
    }

    /// Check if a CONNECT_CFM retransmission is needed (initiator side).
    ///
    /// After transitioning to Established, if no data ACK is received within
    /// 2x RTO, the CFM may have been lost. Returns the cached CFM PDU if a
    /// retry is warranted and the retry limit has not been exceeded.
    pub fn maybe_retry_cfm(&mut self, rto: Duration) -> Option<MacPdu> {
        if self.state != SessionState::Established {
            return None;
        }
        if self.cfm_retries >= MAX_CFM_RETRIES {
            return None;
        }
        let deadline = rto.saturating_mul(2);
        if self.last_activity.elapsed() >= deadline {
            if let Some(ref cfm) = self.last_cfm {
                self.cfm_retries += 1;
                self.last_activity = Instant::now();
                return Some(cfm.clone());
            }
        }
        None
    }

    /// Acknowledge that the peer has confirmed the session (e.g., first data
    /// frame received). Clears the cached CFM so no more retries are attempted.
    pub fn confirm_established(&mut self) {
        self.last_cfm = None;
    }

    /// Record activity (call when any PDU is sent or received).
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if a keepalive should be sent.
    pub fn needs_keepalive(&self) -> bool {
        self.state == SessionState::Established
            && self.last_activity.elapsed() >= self.keepalive_interval
    }

    /// Check if the session has timed out.
    ///
    /// Uses shorter timeouts for transient states:
    /// - Disconnecting: `DEFAULT_DISCONNECT_TIMEOUT_SECS` (10s)
    /// - Connecting/Accepting (handshake): `HANDSHAKE_TIMEOUT_SECS` (30s)
    /// - Established: full `session_timeout` (120s)
    pub fn is_timed_out(&self) -> bool {
        if self.state == SessionState::Idle {
            return false;
        }
        if self.state == SessionState::Disconnecting {
            return self.last_activity.elapsed()
                >= Duration::from_secs(DEFAULT_DISCONNECT_TIMEOUT_SECS);
        }
        // Handshake states use a shorter timeout to avoid lingering half-open sessions
        if self.state == SessionState::Connecting || self.state == SessionState::Accepting {
            return self.last_activity.elapsed() >= Duration::from_secs(HANDSHAKE_TIMEOUT_SECS);
        }
        self.last_activity.elapsed() >= self.session_timeout
    }

    /// Whether this session is established and ready for data transfer.
    pub fn is_established(&self) -> bool {
        self.state == SessionState::Established
    }

    /// Reset the session back to Idle.
    pub fn reset(&mut self) {
        self.state = SessionState::Idle;
        self.remote_caps = None;
        self.negotiated = None;
    }
}

/// Session manager: tracks multiple concurrent sessions.
#[derive(Debug)]
pub struct SessionManager {
    sessions: Vec<Option<Session>>,
}

impl SessionManager {
    /// Create a new session manager supporting up to 16 sessions.
    pub fn new() -> Self {
        Self {
            sessions: (0..16).map(|_| None).collect(),
        }
    }

    /// Allocate a new session, returning its ID.
    pub fn create(
        &mut self,
        local: Callsign,
        remote: Callsign,
        ssid: u8,
        caps: LinkCapabilities,
    ) -> Result<u8> {
        for id in 0..16u8 {
            if self.sessions[id as usize].is_none() {
                self.sessions[id as usize] = Some(Session::new(id, local, remote, ssid, caps));
                return Ok(id);
            }
        }
        Err(anyhow!("No free session slots"))
    }

    /// Get a reference to a session by ID.
    pub fn get(&self, id: u8) -> Option<&Session> {
        self.sessions.get(id as usize).and_then(|s| s.as_ref())
    }

    /// Get a mutable reference to a session by ID.
    pub fn get_mut(&mut self, id: u8) -> Option<&mut Session> {
        self.sessions.get_mut(id as usize).and_then(|s| s.as_mut())
    }

    /// Remove a session by ID.
    pub fn remove(&mut self, id: u8) {
        if (id as usize) < self.sessions.len() {
            self.sessions[id as usize] = None;
        }
    }

    /// Find a session by remote callsign.
    pub fn find_by_remote(&self, remote: &Callsign) -> Option<&Session> {
        self.sessions.iter().flatten().find(|s| s.remote == *remote)
    }

    /// Find a session by remote callsign (mutable).
    pub fn find_by_remote_mut(&mut self, remote: &Callsign) -> Option<&mut Session> {
        self.sessions
            .iter_mut()
            .flatten()
            .find(|s| s.remote == *remote)
    }

    /// Get all active session IDs.
    pub fn active_sessions(&self) -> Vec<u8> {
        self.sessions
            .iter()
            .enumerate()
            .filter_map(|(id, s)| s.as_ref().map(|_| id as u8))
            .collect()
    }

    /// Clean up timed-out sessions. Returns IDs of removed sessions.
    pub fn cleanup_timed_out(&mut self) -> Vec<u8> {
        let mut removed = Vec::new();
        for id in 0..16u8 {
            let timed_out = self.sessions[id as usize]
                .as_ref()
                .map(|s| s.is_timed_out())
                .unwrap_or(false);
            if timed_out {
                self.sessions[id as usize] = None;
                removed.push(id);
            }
        }
        removed
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_callsign(s: &str) -> Callsign {
        Callsign::new(s).unwrap()
    }

    // ── LinkCapabilities tests ──────────────────────────────────────

    #[test]
    fn test_capabilities_roundtrip() {
        let caps = LinkCapabilities {
            max_payload_size: 200,
            fec_rate_num: 1,
            fec_rate_den: 2,
            arq_window: 8,
            compression: true,
            mcs_index: 3,
        };
        let bytes = caps.to_bytes();
        assert_eq!(bytes.len(), 7);
        let decoded = LinkCapabilities::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, caps);
    }

    #[test]
    fn test_capabilities_default() {
        let caps = LinkCapabilities::default();
        let bytes = caps.to_bytes();
        let decoded = LinkCapabilities::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, caps);
    }

    #[test]
    fn test_capabilities_too_short() {
        assert!(LinkCapabilities::from_bytes(&[0; 6]).is_err());
    }

    #[test]
    fn test_capabilities_negotiate() {
        let a = LinkCapabilities {
            max_payload_size: 200,
            fec_rate_num: 1,
            fec_rate_den: 2,
            arq_window: 8,
            compression: true,
            mcs_index: 5,
        };
        let b = LinkCapabilities {
            max_payload_size: 150,
            fec_rate_num: 2,
            fec_rate_den: 3,
            arq_window: 4,
            compression: false,
            mcs_index: 3,
        };
        let negotiated = a.negotiate(&b);
        assert_eq!(negotiated.max_payload_size, 150); // min
        assert_eq!(negotiated.fec_rate_num, 1); // lower rate
        assert_eq!(negotiated.fec_rate_den, 2);
        assert_eq!(negotiated.arq_window, 4); // min
        assert!(!negotiated.compression); // AND
        assert_eq!(negotiated.mcs_index, 3); // min
    }

    #[test]
    fn test_capabilities_negotiate_symmetric() {
        let a = LinkCapabilities::default();
        let b = LinkCapabilities::default();
        let n = a.negotiate(&b);
        assert_eq!(n, a);
    }

    // ── Session state machine tests ─────────────────────────────────

    #[test]
    fn test_three_way_handshake() {
        let local_caps = LinkCapabilities::default();
        let remote_caps = LinkCapabilities {
            max_payload_size: 150,
            ..LinkCapabilities::default()
        };

        // Initiator
        let mut initiator = Session::new(
            0,
            make_callsign("VK3ABC"),
            make_callsign("W1AW"),
            0,
            local_caps.clone(),
        );

        // Responder
        let mut responder = Session::new(
            0,
            make_callsign("W1AW"),
            make_callsign("VK3ABC"),
            0,
            remote_caps.clone(),
        );

        // Step 1: Initiator sends CONNECT_REQ
        let req_pdu = initiator.initiate().unwrap();
        assert_eq!(initiator.state, SessionState::Connecting);
        assert_eq!(req_pdu.frame_type, MacFrameType::ConnectReq);

        // Step 2: Responder handles CONNECT_REQ, sends CONNECT_ACK
        let ack_pdu = responder.handle_connect_req(&req_pdu.payload).unwrap();
        assert_eq!(responder.state, SessionState::Accepting);
        assert_eq!(ack_pdu.frame_type, MacFrameType::ConnectAck);

        // Step 3: Initiator handles CONNECT_ACK, sends CONNECT_CFM
        let cfm_pdu = initiator.handle_connect_ack(&ack_pdu.payload).unwrap();
        assert_eq!(initiator.state, SessionState::Established);
        assert_eq!(cfm_pdu.frame_type, MacFrameType::ConnectCfm);

        // Step 4: Responder handles CONNECT_CFM
        responder.handle_connect_cfm(&cfm_pdu.payload).unwrap();
        assert_eq!(responder.state, SessionState::Established);

        // Both should have the same negotiated capabilities
        let init_neg = initiator.negotiated.as_ref().unwrap();
        let resp_neg = responder.negotiated.as_ref().unwrap();
        assert_eq!(init_neg, resp_neg);
        assert_eq!(init_neg.max_payload_size, 150); // min of 200 and 150
    }

    #[test]
    fn test_cannot_initiate_twice() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        s.initiate().unwrap();
        assert!(s.initiate().is_err());
    }

    #[test]
    fn test_connect_ack_wrong_state() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        let caps_bytes = LinkCapabilities::default().to_bytes();
        assert!(s.handle_connect_ack(&caps_bytes).is_err());
    }

    #[test]
    fn test_connect_req_wrong_state() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        s.initiate().unwrap();
        let caps_bytes = LinkCapabilities::default().to_bytes();
        assert!(s.handle_connect_req(&caps_bytes).is_err());
    }

    #[test]
    fn test_connect_cfm_wrong_state() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        let caps_bytes = LinkCapabilities::default().to_bytes();
        assert!(s.handle_connect_cfm(&caps_bytes).is_err());
    }

    // ── Disconnect tests ────────────────────────────────────────────

    #[test]
    fn test_disconnect() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        s.initiate().unwrap();

        let disc_pdu = s.disconnect().unwrap();
        assert_eq!(disc_pdu.frame_type, MacFrameType::Disconnect);
        assert_eq!(s.state, SessionState::Disconnecting);
    }

    #[test]
    fn test_disconnect_idle() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        assert!(s.disconnect().is_err());
    }

    #[test]
    fn test_handle_disconnect() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        s.initiate().unwrap();
        s.handle_disconnect();
        assert_eq!(s.state, SessionState::Idle);
        assert!(s.negotiated.is_none());
    }

    // ── Keepalive tests ─────────────────────────────────────────────

    #[test]
    fn test_keepalive_not_established() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        assert!(s.keepalive().is_err());
    }

    #[test]
    fn test_is_established() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        assert!(!s.is_established());
        // Manually set for testing
        s.state = SessionState::Established;
        assert!(s.is_established());
    }

    #[test]
    fn test_reset() {
        let mut s = Session::new(
            0,
            make_callsign("AA1AA"),
            make_callsign("BB2BB"),
            0,
            LinkCapabilities::default(),
        );
        s.state = SessionState::Established;
        s.negotiated = Some(LinkCapabilities::default());
        s.reset();
        assert_eq!(s.state, SessionState::Idle);
        assert!(s.negotiated.is_none());
    }

    // ── SessionManager tests ────────────────────────────────────────

    #[test]
    fn test_manager_create_and_get() {
        let mut mgr = SessionManager::new();
        let id = mgr
            .create(
                make_callsign("AA1AA"),
                make_callsign("BB2BB"),
                0,
                LinkCapabilities::default(),
            )
            .unwrap();
        assert_eq!(id, 0);

        let session = mgr.get(id).unwrap();
        assert_eq!(session.local.as_str(), "AA1AA");
        assert_eq!(session.remote.as_str(), "BB2BB");
    }

    #[test]
    fn test_manager_multiple_sessions() {
        let mut mgr = SessionManager::new();
        for i in 0..16u8 {
            // Use valid callsign chars
            let remote_cs = make_callsign(&format!("R{}", i));
            let id = mgr
                .create(
                    make_callsign("LOCAL"),
                    remote_cs,
                    0,
                    LinkCapabilities::default(),
                )
                .unwrap();
            assert_eq!(id, i);
        }

        // 17th should fail
        assert!(mgr
            .create(
                make_callsign("LOCAL"),
                make_callsign("R16"),
                0,
                LinkCapabilities::default(),
            )
            .is_err());
    }

    #[test]
    fn test_manager_remove() {
        let mut mgr = SessionManager::new();
        let id = mgr
            .create(
                make_callsign("A"),
                make_callsign("B"),
                0,
                LinkCapabilities::default(),
            )
            .unwrap();
        assert!(mgr.get(id).is_some());
        mgr.remove(id);
        assert!(mgr.get(id).is_none());
    }

    #[test]
    fn test_manager_find_by_remote() {
        let mut mgr = SessionManager::new();
        mgr.create(
            make_callsign("LOCAL"),
            make_callsign("REMOTE"),
            0,
            LinkCapabilities::default(),
        )
        .unwrap();

        let remote = make_callsign("REMOTE");
        assert!(mgr.find_by_remote(&remote).is_some());

        let unknown = make_callsign("UNKNOWN");
        assert!(mgr.find_by_remote(&unknown).is_none());
    }

    #[test]
    fn test_manager_active_sessions() {
        let mut mgr = SessionManager::new();
        mgr.create(
            make_callsign("L"),
            make_callsign("R1"),
            0,
            LinkCapabilities::default(),
        )
        .unwrap();
        mgr.create(
            make_callsign("L"),
            make_callsign("R2"),
            0,
            LinkCapabilities::default(),
        )
        .unwrap();

        let active = mgr.active_sessions();
        assert_eq!(active, vec![0, 1]);
    }

    #[test]
    fn test_manager_default() {
        let mgr = SessionManager::default();
        assert!(mgr.active_sessions().is_empty());
    }

    // ── Full handshake through manager ──────────────────────────────

    #[test]
    fn test_full_handshake_through_manager() {
        let mut mgr_a = SessionManager::new();
        let mut mgr_b = SessionManager::new();

        let caps_a = LinkCapabilities {
            max_payload_size: 200,
            arq_window: 8,
            ..LinkCapabilities::default()
        };
        let caps_b = LinkCapabilities {
            max_payload_size: 150,
            arq_window: 4,
            ..LinkCapabilities::default()
        };

        // A creates session and initiates
        let id_a = mgr_a
            .create(make_callsign("ALPHA"), make_callsign("BRAVO"), 0, caps_a)
            .unwrap();
        let req = mgr_a.get_mut(id_a).unwrap().initiate().unwrap();

        // B creates session and handles request
        let id_b = mgr_b
            .create(make_callsign("BRAVO"), make_callsign("ALPHA"), 0, caps_b)
            .unwrap();
        let ack = mgr_b
            .get_mut(id_b)
            .unwrap()
            .handle_connect_req(&req.payload)
            .unwrap();

        // A handles ACK
        let cfm = mgr_a
            .get_mut(id_a)
            .unwrap()
            .handle_connect_ack(&ack.payload)
            .unwrap();

        // B handles CFM
        mgr_b
            .get_mut(id_b)
            .unwrap()
            .handle_connect_cfm(&cfm.payload)
            .unwrap();

        // Both established
        assert!(mgr_a.get(id_a).unwrap().is_established());
        assert!(mgr_b.get(id_b).unwrap().is_established());

        // Both have same negotiated params
        let neg_a = mgr_a.get(id_a).unwrap().negotiated.as_ref().unwrap();
        let neg_b = mgr_b.get(id_b).unwrap().negotiated.as_ref().unwrap();
        assert_eq!(neg_a, neg_b);
        assert_eq!(neg_a.max_payload_size, 150);
        assert_eq!(neg_a.arq_window, 4);
    }
}
