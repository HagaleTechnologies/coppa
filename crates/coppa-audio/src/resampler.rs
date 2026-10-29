//! Sample rate conversion wrappers for bridging hardware and engine rates.
//!
//! Most sound cards don't support 8kHz natively. These wrappers sit between
//! the CPAL backend (running at the hardware rate) and the engine (expecting
//! a target rate like 8kHz), resampling transparently in both directions.

use anyhow::{anyhow, Result};
use rubato::{FftFixedIn, FftFixedOut, VecResampler};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

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
        if hw_rate == 0 || target_rate == 0 {
            return Err(anyhow!("audio sample rate must be positive"));
        }
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
        if buf.is_empty() {
            return Ok(0);
        }
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
    pending: Vec<f32>,
    pending_offset: usize,
    generated_hw: u64,
    accepted_engine: u64,
    segment_start_hw: u64,
    segment_start_engine: u64,
    segment_real: u64,
    delay_hw: u64,
    ledger: RefCell<VecDeque<(u64, u64)>>,
    progress_base_engine: Cell<u64>,
}

impl<S: crate::AudioSink> ResamplingSink<S> {
    pub fn new(inner: S, target_rate: u32) -> Result<Self> {
        let hw_rate = inner.sample_rate();
        if hw_rate == 0 || target_rate == 0 {
            return Err(anyhow!("audio sample rate must be positive"));
        }
        let chunk_size = 1024;

        let resampler = FftFixedIn::<f32>::new(
            target_rate as usize,
            hw_rate as usize,
            chunk_size,
            1, // sub_chunks
            1, // channels
        )
        .map_err(|e| anyhow::anyhow!("Failed to create resampler: {}", e))?;
        let delay_hw = resampler.output_delay() as u64;

        Ok(Self {
            inner,
            resampler,
            hw_rate,
            target_rate,
            input_buf: Vec::new(),
            chunk_size,
            pending: Vec::new(),
            pending_offset: 0,
            generated_hw: 0,
            accepted_engine: 0,
            segment_start_hw: 0,
            segment_start_engine: 0,
            segment_real: 0,
            delay_hw,
            ledger: RefCell::new(VecDeque::new()),
            progress_base_engine: Cell::new(0),
        })
    }

    fn drain_pending(&mut self) -> Result<()> {
        while self.pending_offset < self.pending.len() {
            let n = self.inner.write(&self.pending[self.pending_offset..])?;
            if n == 0 {
                break;
            }
            self.pending_offset += n;
        }
        if self.pending_offset == self.pending.len() {
            self.pending.clear();
            self.pending_offset = 0;
        }
        Ok(())
    }

    fn convert_chunk(&mut self) -> Result<()> {
        debug_assert!(self.pending.is_empty());
        self.input_buf.resize(self.chunk_size, 0.0);
        let output = self
            .resampler
            .process(std::slice::from_ref(&self.input_buf), None)
            .map_err(|e| anyhow!("Resample error: {e}"))?;
        self.pending = output[0].clone();
        self.generated_hw += self.pending.len() as u64;
        let mapped = self.map_engine(self.generated_hw);
        if !self.pending.is_empty() {
            self.ledger.get_mut().push_back((self.generated_hw, mapped));
        }
        self.input_buf.clear();
        self.drain_pending()
    }

    fn map_engine(&self, hw_count: u64) -> u64 {
        let segment_hw = hw_count.saturating_sub(self.segment_start_hw);
        let real = segment_hw
            .saturating_sub(self.delay_hw)
            .saturating_mul(self.target_rate as u64)
            / self.hw_rate as u64;
        self.segment_start_engine + real.min(self.segment_real)
    }

    /// Finalize a partial block. This may be called between transmissions.
    pub fn flush(&mut self) -> Result<()> {
        if self.hw_rate == self.target_rate || self.segment_real == 0 {
            return Ok(());
        }
        // The device callback drains the bounded inner queue; wait only for
        // the pending converted block to be accepted, never for playback.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !self.pending.is_empty() {
            self.drain_pending()?;
            if self.pending.is_empty() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "audio output stalled while finalizing resampled tail"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if !self.input_buf.is_empty() {
            self.convert_chunk()?;
            while !self.pending.is_empty() {
                self.drain_pending()?;
                if self.pending.is_empty() {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(anyhow!(
                        "audio output stalled while finalizing resampled tail"
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        let needed_hw = self.segment_start_hw
            + self
                .segment_real
                .saturating_mul(self.hw_rate as u64)
                .div_ceil(self.target_rate as u64)
            + self.delay_hw;
        while self.generated_hw < needed_hw {
            self.input_buf.resize(self.chunk_size, 0.0);
            self.convert_chunk()?;
            while !self.pending.is_empty() {
                self.drain_pending()?;
                if self.pending.is_empty() {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(anyhow!(
                        "audio output stalled while finalizing resampled tail"
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        rubato::Resampler::reset(&mut self.resampler);
        self.segment_start_hw = self.generated_hw;
        self.segment_start_engine = self.accepted_engine;
        self.segment_real = 0;
        Ok(())
    }

    /// Convert a hardware progress snapshot to completed engine samples.
    /// A partially consumed conversion block earns no engine progress.
    fn mapped_at(&self, hw: u64) -> u64 {
        let mut ledger = self.ledger.borrow_mut();
        while ledger.len() > 1 && ledger.front().is_some_and(|(end, _)| *end <= hw) {
            if let Some((_, engine)) = ledger.pop_front() {
                self.progress_base_engine.set(engine);
            }
        }
        let mut engine = self.progress_base_engine.get();
        for &(hw_end, engine_end) in ledger.iter() {
            if hw_end > hw {
                break;
            }
            engine = engine_end;
        }
        engine
    }

    pub fn progress_from_hardware(
        &self,
        progress: crate::PlaybackProgress,
    ) -> crate::PlaybackProgress {
        crate::PlaybackProgress {
            consumed_samples: self.mapped_at(progress.consumed_samples),
            remaining: progress.remaining,
        }
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
    pub fn inner(&self) -> &S {
        &self.inner
    }
}

impl<S: crate::AudioSink> crate::AudioSink for ResamplingSink<S> {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        if self.hw_rate == self.target_rate {
            return self.inner.write(samples);
        }

        self.drain_pending()?;
        if !self.pending.is_empty() {
            return Ok(0);
        }
        let mut accepted = 0;
        while accepted < samples.len() {
            let take = (self.chunk_size - self.input_buf.len()).min(samples.len() - accepted);
            self.input_buf
                .extend_from_slice(&samples[accepted..accepted + take]);
            accepted += take;
            self.accepted_engine += take as u64;
            self.segment_real += take as u64;
            if self.input_buf.len() == self.chunk_size {
                self.convert_chunk()?;
                if !self.pending.is_empty() {
                    break;
                }
            }
        }
        Ok(accepted)
    }

    fn sample_rate(&self) -> u32 {
        self.target_rate
    }

    fn start(&mut self) -> Result<()> {
        self.inner.start()?;
        self.input_buf.clear();
        self.pending.clear();
        self.pending_offset = 0;
        self.generated_hw = 0;
        self.accepted_engine = 0;
        self.segment_start_hw = 0;
        self.segment_start_engine = 0;
        self.segment_real = 0;
        rubato::Resampler::reset(&mut self.resampler);
        self.ledger.get_mut().clear();
        self.progress_base_engine.set(0);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.flush()?;
        self.inner.stop()
    }

    fn finish_input(&mut self) -> Result<()> {
        self.flush()
    }

    /// Drops the not-yet-resampled input and the inner sink's buffer. The
    /// count is in input (target-rate) samples, matching what `write`
    /// accepted.
    fn discard_buffered(&mut self) -> Result<Option<u64>> {
        // The caller does not write while the drop is pending, so the
        // retained input is unchanged until the inner sink reports done.
        let Some(inner) = self.inner.discard_buffered()? else {
            return Ok(None);
        };
        let held = self.input_buf.len() as u64;
        self.input_buf.clear();
        let pending_hw = (self.pending.len() - self.pending_offset) as u64;
        self.pending.clear();
        self.pending_offset = 0;
        let consumed_hw = self.generated_hw.saturating_sub(pending_hw + inner);
        let played_engine = self.mapped_at(consumed_hw);
        let dropped = self.accepted_engine.saturating_sub(played_engine);
        debug_assert!(dropped >= held);
        self.generated_hw = consumed_hw;
        self.accepted_engine = played_engine;
        self.segment_start_hw = consumed_hw;
        self.segment_start_engine = played_engine;
        self.segment_real = 0;
        self.progress_base_engine.set(played_engine);
        rubato::Resampler::reset(&mut self.resampler);
        self.ledger.get_mut().clear();
        Ok(Some(dropped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loopback::LoopbackBackend;
    use crate::{AudioSink, AudioSource};
    use std::collections::VecDeque;

    struct CappedSink {
        rate: u32,
        cap: usize,
        played: Vec<f32>,
    }
    impl AudioSink for CappedSink {
        fn write(&mut self, samples: &[f32]) -> Result<usize> {
            let n = samples.len().min(self.cap);
            self.played.extend_from_slice(&samples[..n]);
            Ok(n)
        }
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    struct FailingSink;
    impl AudioSink for FailingSink {
        fn write(&mut self, _: &[f32]) -> Result<usize> {
            Err(anyhow!("device write failed"))
        }
        fn sample_rate(&self) -> u32 {
            44100
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    struct ChunkedSource {
        rate: u32,
        chunks: VecDeque<Vec<f32>>,
    }
    impl AudioSource for ChunkedSource {
        fn read(&mut self, out: &mut [f32]) -> Result<usize> {
            let Some(mut chunk) = self.chunks.pop_front() else {
                return Ok(0);
            };
            let n = out.len().min(chunk.len());
            out[..n].copy_from_slice(&chunk[..n]);
            if n < chunk.len() {
                chunk.drain(..n);
                self.chunks.push_front(chunk);
            }
            Ok(n)
        }
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn rejects_zero_rates() {
        assert!(ResamplingSink::new(
            CappedSink {
                rate: 0,
                cap: 1,
                played: Vec::new()
            },
            48000
        )
        .is_err());
        assert!(ResamplingSink::new(
            CappedSink {
                rate: 44100,
                cap: 1,
                played: Vec::new()
            },
            0
        )
        .is_err());
        assert!(ResamplingSource::new(
            ChunkedSource {
                rate: 0,
                chunks: VecDeque::new()
            },
            48000
        )
        .is_err());
    }

    #[test]
    fn nonblocking_source_keeps_partial_hardware_chunk() {
        let source = ChunkedSource {
            rate: 44100,
            chunks: VecDeque::from([vec![0.5; 600], Vec::new(), vec![0.5; 2000]]),
        };
        let mut wrapped = ResamplingSource::new(source, 48000).unwrap();
        let mut out = vec![0.0; 2048];
        assert_eq!(wrapped.read(&mut out).unwrap(), 0);
        assert_eq!(wrapped.hw_buf.len(), 600);
        let n = wrapped.read(&mut out).unwrap();
        assert!(n > 0);
        assert!(out[..n].iter().any(|s| s.abs() > 0.01));
    }

    #[test]
    fn bounded_sink_retries_short_writes_and_finishes_partial_tail() {
        for rate in [44100, 8000] {
            let input: Vec<f32> = (0..2501).map(|x| (x as f32 / 32.0).sin()).collect();
            let mut reference = ResamplingSink::new(
                CappedSink {
                    rate,
                    cap: usize::MAX,
                    played: Vec::new(),
                },
                48000,
            )
            .unwrap();
            reference.start().unwrap();
            assert_eq!(reference.write(&input).unwrap(), input.len());
            reference.finish_input().unwrap();
            let expected = reference.inner().played.clone();

            let mut bounded = ResamplingSink::new(
                CappedSink {
                    rate,
                    cap: 0,
                    played: Vec::new(),
                },
                48000,
            )
            .unwrap();
            bounded.start().unwrap();
            let first = bounded.write(&input).unwrap();
            assert!((1024..=2048).contains(&first));
            assert_eq!(bounded.write(&input[first..]).unwrap(), 0);
            assert!(
                bounded.pending.len() <= 2048,
                "converted pending block stays bounded"
            );
            bounded.inner_mut().cap = 37;
            let mut accepted = first;
            while accepted < input.len() {
                accepted += bounded.write(&input[accepted..]).unwrap();
            }
            bounded.finish_input().unwrap();
            assert_eq!(bounded.inner().played, expected, "rate {rate}");
            let before_end = bounded.progress_from_hardware(crate::PlaybackProgress {
                consumed_samples: expected.len() as u64 - 1,
                remaining: std::time::Duration::ZERO,
            });
            assert!(before_end.consumed_samples < input.len() as u64);
            let end = bounded.progress_from_hardware(crate::PlaybackProgress {
                consumed_samples: expected.len() as u64,
                remaining: std::time::Duration::ZERO,
            });
            assert!(end.has_played(input.len() as u64));
        }
    }

    #[test]
    fn discard_drops_blocked_converted_audio_before_next_transmission() {
        let mut sink = ResamplingSink::new(
            CappedSink {
                rate: 44100,
                cap: 0,
                played: Vec::new(),
            },
            48000,
        )
        .unwrap();
        sink.start().unwrap();
        let first = sink.write(&[0.8; 4096]).unwrap();
        assert!((1024..=2048).contains(&first));
        assert_eq!(sink.write(&[0.7; 100]).unwrap(), 0);
        assert_eq!(sink.discard_buffered().unwrap(), Some(first as u64));
        sink.inner_mut().cap = usize::MAX;
        assert_eq!(sink.write(&[0.2; 75]).unwrap(), 75);
        sink.finish_input().unwrap();
        assert!(!sink.inner().played.is_empty());
        assert_eq!(
            sink.progress_from_hardware(crate::PlaybackProgress {
                consumed_samples: sink.inner().played.len() as u64,
                remaining: std::time::Duration::ZERO
            })
            .consumed_samples,
            75
        );
    }

    #[test]
    fn partial_block_discard_and_inner_write_errors_are_visible() {
        let mut partial = ResamplingSink::new(
            CappedSink {
                rate: 44100,
                cap: 0,
                played: Vec::new(),
            },
            48000,
        )
        .unwrap();
        partial.start().unwrap();
        assert_eq!(partial.write(&[0.2; 77]).unwrap(), 77);
        assert_eq!(partial.discard_buffered().unwrap(), Some(77));
        partial.inner_mut().cap = usize::MAX;
        assert_eq!(partial.write(&[0.3; 123]).unwrap(), 123);
        partial.finish_input().unwrap();
        assert_eq!(
            partial
                .progress_from_hardware(crate::PlaybackProgress {
                    consumed_samples: partial.inner().played.len() as u64,
                    remaining: std::time::Duration::ZERO
                })
                .consumed_samples,
            123
        );

        let mut failing = ResamplingSink::new(FailingSink, 48000).unwrap();
        failing.start().unwrap();
        let err = failing.write(&[0.4; 4096]).unwrap_err();
        assert!(err.to_string().contains("device write failed"));
    }

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
        let max_amplitude = recovered[..n]
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_amplitude > 0.001,
            "Recovered signal should have non-zero amplitude, got {}",
            max_amplitude
        );
    }
}
