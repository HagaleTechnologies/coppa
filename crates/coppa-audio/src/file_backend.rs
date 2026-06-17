//! File-based audio backends for regression testing.

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::{AudioSink, AudioSource};

/// Read audio samples from a WAV file.
pub struct WavSource {
    samples: Vec<f32>,
    position: usize,
    sample_rate: u32,
}

/// Write audio samples to a WAV file.
pub struct WavSink {
    samples: Vec<f32>,
    sample_rate: u32,
    path: std::path::PathBuf,
}

/// Read raw f32 samples from a binary file.
pub struct RawF32Source {
    samples: Vec<f32>,
    position: usize,
    sample_rate: u32,
}

/// Write raw f32 samples to a binary file.
pub struct RawF32Sink {
    samples: Vec<f32>,
    sample_rate: u32,
    path: std::path::PathBuf,
}

impl WavSource {
    /// Open a WAV file for reading.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let reader = hound::WavReader::open(path.as_ref())
            .map_err(|e| anyhow!("Failed to open WAV file: {}", e))?;
        let spec = reader.spec();
        let sample_rate = spec.sample_rate;

        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .into_samples::<f32>()
                .map(|s| s.unwrap_or(0.0))
                .collect(),
            hound::SampleFormat::Int => {
                let max_val = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .map(|s| s.unwrap_or(0) as f32 / max_val)
                    .collect()
            }
        };

        // Extract only channel 0 from multi-channel WAV files
        let channels = spec.channels as usize;
        let samples = if channels > 1 {
            samples.into_iter().step_by(channels).collect()
        } else {
            samples
        };

        Ok(Self {
            samples,
            position: 0,
            sample_rate,
        })
    }

    /// Create a WavSource from an in-memory sample buffer (for testing).
    pub fn from_samples(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            samples,
            position: 0,
            sample_rate,
        }
    }

    /// Total number of samples in the file.
    pub fn total_samples(&self) -> usize {
        self.samples.len()
    }

    /// Reset the read position to the beginning.
    pub fn rewind(&mut self) {
        self.position = 0;
    }
}

impl AudioSource for WavSource {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        let remaining = self.samples.len() - self.position;
        let to_read = buf.len().min(remaining);
        buf[..to_read].copy_from_slice(&self.samples[self.position..self.position + to_read]);
        self.position += to_read;
        Ok(to_read)
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

impl WavSink {
    /// Create a new WAV sink that will write to the given path.
    pub fn new<P: AsRef<Path>>(path: P, sample_rate: u32) -> Self {
        Self {
            samples: Vec::new(),
            sample_rate,
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Flush buffered samples to the WAV file.
    pub fn flush_to_file(&self) -> Result<()> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        let mut writer = hound::WavWriter::create(&self.path, spec)
            .map_err(|e| anyhow!("Failed to create WAV file: {}", e))?;

        for &sample in &self.samples {
            writer
                .write_sample(sample)
                .map_err(|e| anyhow!("Failed to write WAV sample: {}", e))?;
        }

        writer
            .finalize()
            .map_err(|e| anyhow!("Failed to finalize WAV file: {}", e))?;

        Ok(())
    }

    /// Get a reference to the buffered samples.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }
}

impl Drop for WavSink {
    fn drop(&mut self) {
        if !self.samples.is_empty() {
            if let Err(e) = self.flush_to_file() {
                eprintln!(
                    "WavSink::drop: failed to flush samples to {}: {}",
                    self.path.display(),
                    e
                );
            }
        }
    }
}

impl AudioSink for WavSink {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        self.samples.extend_from_slice(samples);
        Ok(samples.len())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.flush_to_file()
    }
}

impl RawF32Source {
    /// Open a raw f32 binary file for reading.
    pub fn open<P: AsRef<Path>>(path: P, sample_rate: u32) -> Result<Self> {
        let bytes =
            std::fs::read(path.as_ref()).map_err(|e| anyhow!("Failed to read raw file: {}", e))?;

        if bytes.len() % 4 != 0 {
            return Err(anyhow!("Raw file size is not a multiple of 4 bytes"));
        }

        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        Ok(Self {
            samples,
            position: 0,
            sample_rate,
        })
    }

    /// Create from in-memory samples (for testing).
    pub fn from_samples(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            samples,
            position: 0,
            sample_rate,
        }
    }
}

impl AudioSource for RawF32Source {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        let remaining = self.samples.len() - self.position;
        let to_read = buf.len().min(remaining);
        buf[..to_read].copy_from_slice(&self.samples[self.position..self.position + to_read]);
        self.position += to_read;
        Ok(to_read)
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

impl RawF32Sink {
    /// Create a new raw f32 sink that will write to the given path.
    pub fn new<P: AsRef<Path>>(path: P, sample_rate: u32) -> Self {
        Self {
            samples: Vec::new(),
            sample_rate,
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Flush buffered samples to the raw file.
    pub fn flush_to_file(&self) -> Result<()> {
        let bytes: Vec<u8> = self.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(&self.path, bytes)
            .map_err(|e| anyhow!("Failed to write raw file: {}", e))?;
        Ok(())
    }

    /// Get a reference to the buffered samples.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }
}

impl AudioSink for RawF32Sink {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        self.samples.extend_from_slice(samples);
        Ok(samples.len())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.flush_to_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wav_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_roundtrip.wav");

        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.001).sin()).collect();

        // Write
        let mut sink = WavSink::new(&path, 48000);
        sink.start().unwrap();
        sink.write(&input).unwrap();
        sink.stop().unwrap();

        // Read
        let mut source = WavSource::open(&path).unwrap();
        assert_eq!(source.sample_rate(), 48000);
        assert_eq!(source.total_samples(), 1000);
        source.start().unwrap();

        let mut output = vec![0.0f32; 1000];
        let read = source.read(&mut output).unwrap();
        assert_eq!(read, 1000);

        for (i, (a, b)) in input.iter().zip(output.iter()).enumerate() {
            assert!((a - b).abs() < 1e-5, "Sample {} differs: {} vs {}", i, a, b);
        }

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_raw_f32_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_roundtrip.raw");

        let input: Vec<f32> = (0..500).map(|i| i as f32 * 0.002).collect();

        // Write
        let mut sink = RawF32Sink::new(&path, 48000);
        sink.start().unwrap();
        sink.write(&input).unwrap();
        sink.stop().unwrap();

        // Read
        let mut source = RawF32Source::open(&path, 48000).unwrap();
        source.start().unwrap();

        let mut output = vec![0.0f32; 500];
        let read = source.read(&mut output).unwrap();
        assert_eq!(read, 500);
        assert_eq!(input, output);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_wav_source_from_samples() {
        let mut source = WavSource::from_samples(vec![1.0, 2.0, 3.0], 44100);
        assert_eq!(source.sample_rate(), 44100);
        assert_eq!(source.total_samples(), 3);

        let mut buf = vec![0.0f32; 3];
        let read = source.read(&mut buf).unwrap();
        assert_eq!(read, 3);
        assert_eq!(buf, vec![1.0, 2.0, 3.0]);

        // Rewind
        source.rewind();
        let read = source.read(&mut buf).unwrap();
        assert_eq!(read, 3);
    }

    #[test]
    fn test_raw_f32_source_from_samples() {
        let mut source = RawF32Source::from_samples(vec![0.5, -0.5], 8000);
        assert_eq!(source.sample_rate(), 8000);

        let mut buf = vec![0.0f32; 2];
        let read = source.read(&mut buf).unwrap();
        assert_eq!(read, 2);
        assert_eq!(buf, vec![0.5, -0.5]);
    }
}
