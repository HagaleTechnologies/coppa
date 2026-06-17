//! No-op PTT for testing or soundcard-only operation.

use crate::{PttControl, PttState};
use anyhow::Result;

/// No-op PTT controller that tracks state without hardware.
pub struct NullPtt {
    state: PttState,
}

impl NullPtt {
    pub fn new() -> Self {
        Self {
            state: PttState::Rx,
        }
    }
}

impl Default for NullPtt {
    fn default() -> Self {
        Self::new()
    }
}

impl PttControl for NullPtt {
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
    fn test_null_ptt_default() {
        let mut ptt = NullPtt::new();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
    }

    #[test]
    fn test_null_ptt_roundtrip() {
        let mut ptt = NullPtt::new();
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx);
        ptt.set_ptt(PttState::Rx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
    }

    #[test]
    fn test_null_ptt_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<NullPtt>();
    }
}
