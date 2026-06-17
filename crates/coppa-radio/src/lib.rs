//! Radio control backends for Coppa.
//!
//! Provides traits and implementations for controlling amateur radio
//! transceivers and PTT (Push-To-Talk) devices.

use anyhow::Result;

pub mod null_ptt;
pub mod rigctld;
pub mod vox_ptt;

#[cfg(feature = "serial-ptt-stub")]
pub mod ptt_serial;

#[cfg(feature = "gpio-ptt-stub")]
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

#[cfg(test)]
mod tests {
    use super::*;

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
