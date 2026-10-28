//! Radio control backends for Coppa.
//!
//! Provides traits and implementations for controlling amateur radio
//! transceivers and PTT (Push-To-Talk) devices.

// Library diagnostics go through `tracing`, never straight to stdout/stderr.
#![cfg_attr(not(test), deny(clippy::print_stdout, clippy::print_stderr))]

use anyhow::Result;

pub mod band;
pub mod null_ptt;
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
