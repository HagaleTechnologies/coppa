//! Radio control backends for Coppa.
//!
//! Provides traits and implementations for controlling amateur radio
//! transceivers and PTT (Push-To-Talk) devices.

// Library diagnostics go through `tracing`, never straight to stdout/stderr.
#![cfg_attr(not(test), deny(clippy::print_stdout, clippy::print_stderr))]

use anyhow::Result;

pub mod band;
pub mod null_ptt;
pub mod ptt_cm108;
pub mod rigctld;
pub mod vox_ptt;

#[cfg(feature = "serial-ptt")]
pub mod ptt_serial;

#[cfg(feature = "gpio-ptt")]
pub mod ptt_gpio;

pub use null_ptt::NullPtt;
pub use rigctld::RigctldClient;
pub use vox_ptt::VoxPtt;

/// Operating mode of the transceiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioMode {
    Usb,
    Lsb,
    Am,
    Fm,
    Digital,
}

/// State of the transceiver's PTT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttState {
    Rx,
    Tx,
}

/// Control interface for an amateur radio transceiver.
pub trait RadioControl: Send {
    fn get_frequency(&mut self) -> Result<u64>;
    fn set_frequency(&mut self, freq_hz: u64) -> Result<()>;
    fn get_mode(&mut self) -> Result<RadioMode>;
    fn set_mode(&mut self, mode: RadioMode) -> Result<()>;
    fn get_ptt(&mut self) -> Result<PttState>;
    fn set_ptt(&mut self, state: PttState) -> Result<()>;
    fn signal_strength(&mut self) -> Result<f32>;
}

/// Standalone PTT control trait.
pub trait PttControl: Send {
    fn set_ptt(&mut self, state: PttState) -> Result<()>;
    fn get_ptt(&mut self) -> Result<PttState>;
}

/// A PTT backend that keeps its concrete type when it can do more than key
/// the transmitter.
///
/// Boxing every backend as `dyn PttControl` erased `RigctldClient`'s
/// `RadioControl` impl, leaving the rig's dial frequency unreachable. The
/// `Rigctld` arm retains the client so [`PttBackend::poll_frequency`] can ask
/// the rig; every other backend has no frequency source and answers `None`.
pub enum PttBackend {
    Rigctld(RigctldClient),
    Other(Box<dyn PttControl>),
}

impl PttBackend {
    /// The rig's current dial frequency, or `None` when this backend has no
    /// frequency source or the rig did not answer. Never a cached or default
    /// value -- see [`RigctldClient::poll_frequency`].
    pub fn poll_frequency(&mut self) -> Option<u64> {
        match self {
            PttBackend::Rigctld(client) => client.poll_frequency(),
            PttBackend::Other(_) => None,
        }
    }

    /// Whether this backend is a connected rigctld client.
    pub fn is_rigctld(&self) -> bool {
        matches!(self, PttBackend::Rigctld(_))
    }

    /// COP-84: the rig's dial frequency, with the reason on failure. Honest,
    /// never cached -- see [`RigctldClient::query_frequency`]. A backend with
    /// no CAT control answers an error without any I/O.
    pub fn query_frequency(&mut self) -> Result<u64> {
        match self {
            PttBackend::Rigctld(client) => client.query_frequency(),
            PttBackend::Other(_) => Err(no_cat_control()),
        }
    }

    /// COP-84: the rig's mode, as the rig's own token (`USB`, `PKTLSB`, ...).
    /// Honest, never cached -- see [`RigctldClient::query_mode`]. A backend
    /// with no CAT control answers an error without any I/O.
    pub fn query_mode(&mut self) -> Result<String> {
        match self {
            PttBackend::Rigctld(client) => client.query_mode(),
            PttBackend::Other(_) => Err(no_cat_control()),
        }
    }

    /// COP-84: tune the rig; `Ok` only when the rig answered `RPRT 0`. Honest,
    /// never cached: read the dial back with [`Self::query_frequency`] -- see
    /// [`RigctldClient::try_set_frequency`]. A backend with no CAT control
    /// answers an error without any I/O.
    pub fn set_frequency(&mut self, hz: u64) -> Result<()> {
        match self {
            PttBackend::Rigctld(client) => client.try_set_frequency(hz),
            PttBackend::Other(_) => Err(no_cat_control()),
        }
    }

    /// COP-84: set the rig's mode to a hamlib token; `Ok` only when the rig
    /// answered `RPRT 0`. Honest, never cached: read it back with
    /// [`Self::query_mode`] -- see [`RigctldClient::try_set_mode`]. A backend
    /// with no CAT control answers an error without any I/O.
    pub fn set_mode(&mut self, token: &str) -> Result<()> {
        match self {
            PttBackend::Rigctld(client) => client.try_set_mode(token),
            PttBackend::Other(_) => Err(no_cat_control()),
        }
    }
}

/// The error every rig-control call gets from a backend that only keys PTT.
fn no_cat_control() -> anyhow::Error {
    anyhow::anyhow!(
        "this PTT backend has no CAT control; rig control needs [radio] ptt_method = \"rigctld\""
    )
}

impl PttControl for PttBackend {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        match self {
            PttBackend::Rigctld(client) => PttControl::set_ptt(client, state),
            PttBackend::Other(ptt) => ptt.set_ptt(state),
        }
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        match self {
            PttBackend::Rigctld(client) => PttControl::get_ptt(client),
            PttBackend::Other(ptt) => ptt.get_ptt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptt_backend_other_has_no_frequency() {
        let mut backend = PttBackend::Other(Box::new(NullPtt::new()));
        assert!(!backend.is_rigctld());
        assert_eq!(backend.poll_frequency(), None);
        backend.set_ptt(PttState::Tx).unwrap();
        assert_eq!(backend.get_ptt().unwrap(), PttState::Tx);
    }

    #[test]
    fn cop84_ptt_backend_other_has_no_cat_control() {
        let mut backend = PttBackend::Other(Box::new(NullPtt::new()));
        let errors = [
            backend.query_frequency().map(|_| ()).unwrap_err(),
            backend.query_mode().map(|_| ()).unwrap_err(),
            backend.set_frequency(7_074_000).unwrap_err(),
            backend.set_mode("USB").unwrap_err(),
        ];
        for error in errors {
            assert!(
                error.to_string().contains("no CAT control"),
                "error: {error}"
            );
        }
    }

    #[test]
    fn test_radio_mode_equality() {
        assert_eq!(RadioMode::Usb, RadioMode::Usb);
        assert_ne!(RadioMode::Usb, RadioMode::Lsb);
    }

    #[test]
    fn test_ptt_state_equality() {
        assert_eq!(PttState::Rx, PttState::Rx);
        assert_ne!(PttState::Rx, PttState::Tx);
    }
}
