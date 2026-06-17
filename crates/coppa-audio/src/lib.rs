//! Audio I/O backends for Coppa.
//!
//! Provides traits and implementations for audio capture and playback,
//! including ring-buffer bridging, loopback testing, VOX detection,
//! and file I/O for regression testing.

use anyhow::Result;

pub mod loopback;
pub mod ringbuf;
pub mod vox;

#[cfg(feature = "cpal-backend")]
pub mod cpal_backend;

#[cfg(feature = "file-backend")]
pub mod file_backend;

pub use loopback::{LoopbackBackend, LoopbackSink, LoopbackSource};
pub use ringbuf::{audio_ring, AudioRingConsumer, AudioRingProducer};
pub use vox::VoxDetector;

#[cfg(feature = "cpal-backend")]
pub use cpal_backend::{CpalSink, CpalSource};

#[cfg(feature = "cpal-backend")]
pub use cpal_backend::{find_input_device_by_name, find_output_device_by_name};

#[cfg(feature = "file-backend")]
pub use file_backend::{RawF32Sink, RawF32Source, WavSink, WavSource};

/// Describes an available audio device.
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub name: String,
    pub max_sample_rate: u32,
    pub input_channels: u16,
    pub output_channels: u16,
}

/// Audio capture source.
pub trait AudioSource: Send {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize>;
    fn sample_rate(&self) -> u32;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}

/// Audio playback sink.
pub trait AudioSink: Send {
    fn write(&mut self, samples: &[f32]) -> Result<usize>;
    fn sample_rate(&self) -> u32;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}

/// Enumerate available audio devices.
#[cfg(feature = "cpal-backend")]
pub fn list_devices() -> Result<Vec<AudioDevice>> {
    cpal_backend::list_cpal_devices()
}

/// Enumerate available audio devices (no backend available).
#[cfg(not(feature = "cpal-backend"))]
pub fn list_devices() -> Result<Vec<AudioDevice>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_device_struct() {
        let dev = AudioDevice {
            name: "Test Device".to_string(),
            max_sample_rate: 48000,
            input_channels: 1,
            output_channels: 2,
        };
        assert_eq!(dev.name, "Test Device");
        assert_eq!(dev.max_sample_rate, 48000);
    }
}
