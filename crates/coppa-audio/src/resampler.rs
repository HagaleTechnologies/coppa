//! Sample rate conversion wrappers for bridging hardware and engine rates.
//!
//! Most sound cards don't support 8kHz natively. These wrappers sit between
//! the CPAL backend (running at the hardware rate) and the engine (expecting
//! a target rate like 8kHz), resampling transparently in both directions.

use anyhow::Result;
use rubato::{FftFixedIn, FftFixedOut, VecResampler};

/// Resampling audio source: reads from an inner source at hardware rate,
/// resamples down to the target (engine) rate.
pub struct ResamplingSource<S> {
    inner: S,
    resampler: FftFixedOut<f32>,
    hw_rate: u32,
    target_rate: u32,
    /// Buffer for reading hardware-rate samples
    hw_buf: Vec<f32>,
    /// Residual output samples from previous resample call
    residual: Vec<f32>,
}

impl<S: crate::AudioSource> ResamplingSource<S> {
    pub fn new(inner: S, target_rate: u32) -> Result<Self> {
        let hw_rate = inner.sample_rate();
        let chunk_size = 1024;
        let resampler = FftFixedOut::<f32>::new(
            hw_rate as usize,
            target_rate as usize,
            chunk_size,
            1, // sub_chunks
            1, // channels
        )
        .map_err(|e| anyhow::anyhow!("Failed to create resampler: {}", e))?;

        Ok(Self {
            inner,
            resampler,
            hw_rate,
            target_rate,
            hw_buf: Vec::new(),
            residual: Vec::new(),
        })
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: crate::AudioSource> crate::AudioSource for ResamplingSource<S> {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        if self.hw_rate == self.target_rate {
            return self.inner.read(buf);
        }

        // Drain residual first
        if !self.residual.is_empty() {
            let n = buf.len().min(self.residual.len());
            buf[..n].copy_from_slice(&self.residual[..n]);
            self.residual.drain(..n);
            return Ok(n);
        }

        // Read enough hardware samples to produce output
        let input_frames_needed = self.resampler.input_frames_next();
        while self.hw_buf.len() < input_frames_needed {
            let need = input_frames_needed - self.hw_buf.len();
            let mut tmp = vec![0.0f32; need.max(1024)];
            let n = self.inner.read(&mut tmp)?;
            if n == 0 {
                return Ok(0);
            }
            self.hw_buf.extend_from_slice(&tmp[..n]);
        }

        // Take exactly what the resampler needs
        let input_chunk: Vec<f32> = self.hw_buf.drain(..input_frames_needed).collect();
        let input_vec = vec![input_chunk];

        let output = self
            .resampler
            .process(&input_vec, None)
            .map_err(|e| anyhow::anyhow!("Resample error: {}", e))?;

        let resampled = &output[0];
        let n = buf.len().min(resampled.len());
        buf[..n].copy_from_slice(&resampled[..n]);

        if resampled.len() > n {
            self.residual.extend_from_slice(&resampled[n..]);
        }

        Ok(n)
    }

    fn sample_rate(&self) -> u32 {
        self.target_rate
    }

    fn start(&mut self) -> Result<()> {
        self.inner.start()
    }

    fn stop(&mut self) -> Result<()> {
        self.inner.stop()
    }
}

/// Resampling audio sink: accepts samples at the target (engine) rate,
/// resamples up to the hardware rate, and writes to the inner sink.
pub struct ResamplingSink<S> {
    inner: S,
    resampler: FftFixedIn<f32>,
    hw_rate: u32,
    target_rate: u32,
    input_buf: Vec<f32>,
    chunk_size: usize,
}

impl<S: crate::AudioSink> ResamplingSink<S> {
    pub fn new(inner: S, target_rate: u32) -> Result<Self> {
        let hw_rate = inner.sample_rate();
        let chunk_size = 1024;

        let resampler = FftFixedIn::<f32>::new(
            target_rate as usize,
            hw_rate as usize,
            chunk_size,
            1, // sub_chunks
            1, // channels
        )
        .map_err(|e| anyhow::anyhow!("Failed to create resampler: {}", e))?;

        Ok(Self {
            inner,
            resampler,
            hw_rate,
            target_rate,
            input_buf: Vec::new(),
            chunk_size,
        })
    }

    pub fn flush(&mut self) -> Result<()> {
        if self.hw_rate == self.target_rate || self.input_buf.is_empty() {
            return Ok(());
        }
        self.input_buf.resize(self.chunk_size, 0.0);
        let input_vec = vec![self.input_buf.clone()];
        if let Ok(output) = self.resampler.process(&input_vec, None) {
            let _ = self.inner.write(&output[0]);
        }
        self.input_buf.clear();
        Ok(())
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: crate::AudioSink> crate::AudioSink for ResamplingSink<S> {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        if self.hw_rate == self.target_rate {
            return self.inner.write(samples);
        }

        self.input_buf.extend_from_slice(samples);

        while self.input_buf.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.input_buf.drain(..self.chunk_size).collect();
            let input_vec = vec![chunk];

            let output = self
                .resampler
                .process(&input_vec, None)
                .map_err(|e| anyhow::anyhow!("Resample error: {}", e))?;

            self.inner.write(&output[0])?;
        }

        Ok(samples.len())
    }

    fn sample_rate(&self) -> u32 {
        self.target_rate
    }

    fn start(&mut self) -> Result<()> {
        self.inner.start()
    }

    fn stop(&mut self) -> Result<()> {
        self.flush()?;
        self.inner.stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loopback::LoopbackBackend;
    use crate::{AudioSink, AudioSource};

    #[test]
    fn test_resampling_source_passthrough() {
        let (source, _sink) = LoopbackBackend::new(8000, 8192);
        let mut rs = ResamplingSource::new(source, 8000).unwrap();
        rs.start().unwrap();
        assert_eq!(rs.sample_rate(), 8000);
    }

    #[test]
    fn test_resampling_sink_passthrough() {
        let (_source, sink) = LoopbackBackend::new(8000, 8192);
        let mut rs = ResamplingSink::new(sink, 8000).unwrap();
        rs.start().unwrap();
        assert_eq!(rs.sample_rate(), 8000);
    }

    #[test]
    fn test_resampling_roundtrip_48k_to_8k() {
        let engine_rate = 8000u32;
        let hw_rate = 48000u32;

        // Generate a sine wave at engine rate
        let original: Vec<f32> = (0..4096)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / engine_rate as f32).sin())
            .collect();

        let (hw_source, hw_sink) = LoopbackBackend::new(hw_rate, 262144);

        let mut tx = ResamplingSink::new(hw_sink, engine_rate).unwrap();
        let mut rx = ResamplingSource::new(hw_source, engine_rate).unwrap();

        tx.start().unwrap();
        rx.start().unwrap();

        // Write engine-rate samples → upsampled to 48k → loopback → downsampled to 8k
        tx.write(&original).unwrap();
        tx.stop().unwrap(); // flush remaining

        let mut recovered = vec![0.0f32; 8192];
        let n = rx.read(&mut recovered).unwrap();

        // Verify we recovered samples and they have non-trivial amplitude
        assert!(n > 0, "Should recover some samples");
        let max_amplitude = recovered[..n].iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_amplitude > 0.001,
            "Recovered signal should have non-zero amplitude, got {}",
            max_amplitude
        );
    }
}
