//! CPAL audio backend for real hardware I/O.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;

use crate::ringbuf::{audio_ring, AudioRingConsumer, AudioRingProducer};
use crate::{AudioDevice, AudioSink, AudioSource};

/// List audio devices via CPAL.
pub fn list_cpal_devices() -> Result<Vec<AudioDevice>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();

    for device in host
        .devices()
        .map_err(|e| anyhow!("Failed to list devices: {}", e))?
    {
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "Unknown".to_string());
        let mut max_sr = 0u32;
        let mut in_ch = 0u16;
        let mut out_ch = 0u16;

        if let Ok(configs) = device.supported_input_configs() {
            for config in configs {
                in_ch = in_ch.max(config.channels());
                max_sr = max_sr.max(config.max_sample_rate());
            }
        }
        if let Ok(configs) = device.supported_output_configs() {
            for config in configs {
                out_ch = out_ch.max(config.channels());
                max_sr = max_sr.max(config.max_sample_rate());
            }
        }

        devices.push(AudioDevice {
            name,
            max_sample_rate: max_sr,
            input_channels: in_ch,
            output_channels: out_ch,
        });
    }

    Ok(devices)
}

/// Wrapper around `cpal::Stream` that allows `Send` with a debug-mode
/// assertion that the stream is dropped on the same thread it was created on.
///
/// # Safety
/// `cpal::Stream` is `!Send` on macOS because CoreAudio ties streams to their
/// creation thread. We mark `SendStream` as `Send` because:
/// 1. The stream is only created in `start()` and dropped in `stop()` / `Drop`
///    on the same async runtime thread.
/// 2. The ring buffer endpoints (`rtrb` Producer/Consumer) are themselves `Send`.
/// 3. In debug builds, a thread-id assertion catches cross-thread drops early.
struct SendStream {
    inner: cpal::Stream,
    #[cfg(debug_assertions)]
    created_on: std::thread::ThreadId,
}

impl SendStream {
    fn new(stream: cpal::Stream) -> Self {
        Self {
            inner: stream,
            #[cfg(debug_assertions)]
            created_on: std::thread::current().id(),
        }
    }
}

impl Drop for SendStream {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            std::thread::current().id(),
            self.created_on,
            "SendStream must be dropped on the same thread it was created on"
        );
    }
}

impl std::ops::Deref for SendStream {
    type Target = cpal::Stream;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// Safety: see SendStream doc comment above.
unsafe impl Send for SendStream {}

/// CPAL-based audio source for capturing audio from hardware.
pub struct CpalSource {
    consumer: AudioRingConsumer,
    sample_rate: u32,
    stream: Option<SendStream>,
    device: cpal::Device,
    config: StreamConfig,
}

/// CPAL-based audio sink for playing audio through hardware.
pub struct CpalSink {
    producer: AudioRingProducer,
    sample_rate: u32,
    stream: Option<SendStream>,
    device: cpal::Device,
    config: StreamConfig,
}

impl CpalSource {
    /// Create a new CPAL source using the default input device.
    ///
    /// You must call `start()` before `read()` will return any data.
    pub fn new(sample_rate: u32, _buffer_size: usize) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("No input device available"))?;

        let config = StreamConfig {
            channels: 1,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Create a placeholder consumer; start() will replace it with a
        // properly wired ring buffer connected to the CPAL callback.
        let (_producer, consumer) = audio_ring(64);

        Ok(Self {
            consumer,
            sample_rate,
            stream: None,
            device,
            config,
        })
    }

    /// Create a CPAL source from a specific device.
    pub fn from_device(
        device: cpal::Device,
        sample_rate: u32,
        _buffer_size: usize,
    ) -> Result<Self> {
        let config = StreamConfig {
            channels: 1,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let (_producer, consumer) = audio_ring(64);

        Ok(Self {
            consumer,
            sample_rate,
            stream: None,
            device,
            config,
        })
    }
}

impl AudioSource for CpalSource {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        Ok(self.consumer.read(buf))
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        let (mut producer, consumer) = audio_ring(8192);
        self.consumer = consumer;

        let stream = self
            .device
            .build_input_stream(
                self.config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    producer.write(data);
                },
                |err| {
                    eprintln!("CPAL input error: {}", err);
                },
                None,
            )
            .map_err(|e| anyhow!("Failed to build input stream: {}", e))?;

        stream
            .play()
            .map_err(|e| anyhow!("Failed to start input stream: {}", e))?;
        self.stream = Some(SendStream::new(stream));
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(stream) = self.stream.take() {
            stream
                .pause()
                .map_err(|e| anyhow!("Failed to pause input stream: {}", e))?;
        }
        Ok(())
    }
}

impl CpalSink {
    /// Create a new CPAL sink using the default output device.
    ///
    /// You must call `start()` before `write()` will output audio.
    pub fn new(sample_rate: u32, _buffer_size: usize) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No output device available"))?;

        let config = StreamConfig {
            channels: 1,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Placeholder producer; start() replaces with properly wired one.
        let (producer, _consumer) = audio_ring(64);

        Ok(Self {
            producer,
            sample_rate,
            stream: None,
            device,
            config,
        })
    }

    /// Create a CPAL sink from a specific device.
    pub fn from_device(
        device: cpal::Device,
        sample_rate: u32,
        _buffer_size: usize,
    ) -> Result<Self> {
        let config = StreamConfig {
            channels: 1,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let (producer, _consumer) = audio_ring(64);

        Ok(Self {
            producer,
            sample_rate,
            stream: None,
            device,
            config,
        })
    }
}

impl AudioSink for CpalSink {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        Ok(self.producer.write(samples))
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        let (producer, mut consumer) = audio_ring(8192);
        self.producer = producer;

        let stream = self
            .device
            .build_output_stream(
                self.config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let read = consumer.read(data);
                    // Fill remaining with silence
                    for sample in &mut data[read..] {
                        *sample = 0.0;
                    }
                },
                |err| {
                    eprintln!("CPAL output error: {}", err);
                },
                None,
            )
            .map_err(|e| anyhow!("Failed to build output stream: {}", e))?;

        stream
            .play()
            .map_err(|e| anyhow!("Failed to start output stream: {}", e))?;
        self.stream = Some(SendStream::new(stream));
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(stream) = self.stream.take() {
            stream
                .pause()
                .map_err(|e| anyhow!("Failed to pause output stream: {}", e))?;
        }
        Ok(())
    }
}

// Safety for CpalSource/CpalSink Send: cpal::Stream is wrapped in SendStream
// (which is Send). cpal::Device may be !Send on some platforms (e.g. macOS
// CoreAudio), but we only use it in start() to build a stream and never
// access it from a different thread concurrently.
unsafe impl Send for CpalSource {}
unsafe impl Send for CpalSink {}

/// Find an input device whose name contains the given substring (case-insensitive).
/// Returns `None` if no match or if `name` is empty.
pub fn find_input_device_by_name(name: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    if name.is_empty() {
        return None;
    }
    let host = cpal::default_host();
    let needle = name.to_lowercase();
    host.input_devices().ok()?.find(|d| {
        d.description()
            .map(|info| info.name().to_lowercase().contains(&needle))
            .unwrap_or(false)
    })
}

/// Find an output device whose name contains the given substring (case-insensitive).
/// Returns `None` if no match or if `name` is empty.
pub fn find_output_device_by_name(name: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    if name.is_empty() {
        return None;
    }
    let host = cpal::default_host();
    let needle = name.to_lowercase();
    host.output_devices().ok()?.find(|d| {
        d.description()
            .map(|info| info.name().to_lowercase().contains(&needle))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_input_device_returns_none_for_nonexistent() {
        let result = find_input_device_by_name("NONEXISTENT_DEVICE_XYZ_12345");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_output_device_returns_none_for_nonexistent() {
        let result = find_output_device_by_name("NONEXISTENT_DEVICE_XYZ_12345");
        assert!(result.is_none());
    }
}
