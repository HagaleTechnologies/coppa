//! PTT driven by an external VOX (Voice Operated Switch) detector.

use crate::{PttControl, PttState};
use anyhow::Result;

/// PTT controlled by an external VOX state.
///
/// The VOX state is set externally (e.g., by the audio pipeline's
/// VoxDetector) and this adaptor provides PttControl over it.
pub struct VoxPtt {
    state: PttState,
}

impl VoxPtt {
    pub fn new() -> Self {
        Self {
            state: PttState::Rx,
        }
    }

    /// Update the PTT state based on VOX detection.
    pub fn update_from_vox(&mut self, vox_active: bool) {
        self.state = if vox_active {
            PttState::Tx
        } else {
            PttState::Rx
        };
    }
}

impl Default for VoxPtt {
    fn default() -> Self {
        Self::new()
    }
}

impl PttControl for VoxPtt {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        self.state = state;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vox_ptt_default() {
        let mut ptt = VoxPtt::new();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
    }

    #[test]
    fn test_vox_ptt_update() {
        let mut ptt = VoxPtt::new();
        ptt.update_from_vox(true);
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx);
        ptt.update_from_vox(false);
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
    }
}
