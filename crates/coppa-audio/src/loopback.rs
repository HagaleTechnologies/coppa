//! Loopback audio backend for testing.
//!
//! Connects a sink directly to a source via a ring buffer,
//! optionally applying a channel model (e.g., AWGN).

use crate::{
    ringbuf::{audio_ring, AudioRingConsumer, AudioRingProducer},
    AudioSink, AudioSource,
};
use anyhow::Result;

/// Channel model function applied between loopback sink and source.
pub type ChannelModelFn = Box<dyn Fn(&[f32]) -> Vec<f32> + Send>;

/// Loopback source — reads samples written to the paired sink.
pub struct LoopbackSource {
    consumer: AudioRingConsumer,
    sample_rate: u32,
    started: bool,
}

/// Loopback sink — writes samples that become available on the paired source.
pub struct LoopbackSink {
    producer: AudioRingProducer,
    sample_rate: u32,
    started: bool,
    channel_model: Option<ChannelModelFn>,
}

/// Create a loopback backend pair.
pub struct LoopbackBackend;

impl LoopbackBackend {
    /// Create a loopback source/sink pair with the given sample rate and buffer size.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(sample_rate: u32, buffer_size: usize) -> (LoopbackSource, LoopbackSink) {
        let (producer, consumer) = audio_ring(buffer_size);
        (
            LoopbackSource {
                consumer,
                sample_rate,
                started: false,
            },
            LoopbackSink {
                producer,
                sample_rate,
                started: false,
                channel_model: None,
            },
        )
    }

    /// Create a loopback pair with an optional channel model applied to written samples.
    pub fn with_channel_model(
        sample_rate: u32,
        buffer_size: usize,
        model: ChannelModelFn,
    ) -> (LoopbackSource, LoopbackSink) {
        let (producer, consumer) = audio_ring(buffer_size);
        (
            LoopbackSource {
                consumer,
                sample_rate,
                started: false,
            },
            LoopbackSink {
                producer,
                sample_rate,
                started: false,
                channel_model: Some(model),
            },
        )
    }
}

impl AudioSource for LoopbackSource {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        Ok(self.consumer.read(buf))
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.started = false;
        Ok(())
    }
}

impl AudioSink for LoopbackSink {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        let data = if let Some(ref model) = self.channel_model {
            model(samples)
        } else {
            samples.to_vec()
        };
        Ok(self.producer.write(&data))
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.started = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loopback_roundtrip() {
        let (mut source, mut sink) = LoopbackBackend::new(48000, 4096);
        source.start().unwrap();
        sink.start().unwrap();

        let input: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
        let written = sink.write(&input).unwrap();
        assert_eq!(written, 100);

        let mut output = vec![0.0f32; 100];
        let read = source.read(&mut output).unwrap();
        assert_eq!(read, 100);
        assert_eq!(input, output);
    }

    #[test]
    fn test_loopback_with_channel_model() {
        // Apply simple gain as channel model
        let model: ChannelModelFn =
            Box::new(|samples: &[f32]| samples.iter().map(|s| s * 0.5).collect());
        let (mut source, mut sink) = LoopbackBackend::with_channel_model(48000, 4096, model);
        source.start().unwrap();
        sink.start().unwrap();

        let input = vec![1.0, 2.0, 3.0, 4.0];
        sink.write(&input).unwrap();

        let mut output = vec![0.0f32; 4];
        source.read(&mut output).unwrap();
        assert_eq!(output, vec![0.5, 1.0, 1.5, 2.0]);
    }

    #[test]
    fn test_loopback_sample_rate() {
        let (source, sink) = LoopbackBackend::new(44100, 1024);
        assert_eq!(source.sample_rate(), 44100);
        assert_eq!(sink.sample_rate(), 44100);
    }

    #[test]
    fn test_loopback_start_stop() {
        let (mut source, mut sink) = LoopbackBackend::new(48000, 1024);
        assert!(!source.started);
        assert!(!sink.started);
        source.start().unwrap();
        sink.start().unwrap();
        assert!(source.started);
        assert!(sink.started);
        source.stop().unwrap();
        sink.stop().unwrap();
        assert!(!source.started);
        assert!(!sink.started);
    }
}
