//! Linux sysfs GPIO PTT control.

use crate::{PttControl, PttState};
use anyhow::Result;

/// GPIO PTT using Linux sysfs interface.
///
/// Controls a GPIO pin for PTT signaling. On non-Linux platforms,
/// this tracks state without hardware access.
pub struct GpioPtt {
    gpio_pin: u32,
    state: PttState,
    inverted: bool,
}

impl GpioPtt {
    /// Create a GPIO PTT controller.
    ///
    /// * `gpio_pin` - GPIO pin number (BCM numbering on Raspberry Pi)
    /// * `inverted` - If true, PTT=TX drives the pin LOW
    pub fn new(gpio_pin: u32, inverted: bool) -> Result<Self> {
        Ok(Self {
            gpio_pin,
            state: PttState::Rx,
            inverted,
        })
    }

    /// Get the configured GPIO pin number.
    pub fn pin(&self) -> u32 {
        self.gpio_pin
    }

    /// Whether the logic is inverted.
    pub fn is_inverted(&self) -> bool {
        self.inverted
    }
}

impl PttControl for GpioPtt {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        // STUB: requires Linux sysfs GPIO and physical hardware.
        //
        // Real implementation would:
        //   1. Export the pin: write self.gpio_pin to /sys/class/gpio/export
        //   2. Set direction: write "out" to /sys/class/gpio/gpio{N}/direction
        //   3. Determine value based on state and self.inverted:
        //      - TX + not inverted => "1", TX + inverted => "0"
        //      - RX + not inverted => "0", RX + inverted => "1"
        //   4. Write value to /sys/class/gpio/gpio{N}/value
        //   5. On Drop, unexport: write pin to /sys/class/gpio/unexport
        self.state = state;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        Ok(self.state)
    }
}

impl Drop for GpioPtt {
    fn drop(&mut self) {
        // Ensure PTT is released on drop
        self.state = PttState::Rx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpio_ptt_create() {
        let ptt = GpioPtt::new(17, false).unwrap();
        assert_eq!(ptt.pin(), 17);
        assert!(!ptt.is_inverted());
    }

    #[test]
    fn test_gpio_ptt_roundtrip() {
        let mut ptt = GpioPtt::new(27, true).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx);
        ptt.set_ptt(PttState::Rx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
    }
}
