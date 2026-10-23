//! CPAL audio backend for real hardware I/O.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;

use std::sync::Arc;
use std::time::Instant;

use crate::playout::{fill_output, CallbackState, PlaybackProgress, ProgressShared};
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
///
/// `write` is nonblocking and lossless from the caller's point of view: it
/// accepts at most the TX ring's free space and returns that count, so a
/// caller retains and retries the unwritten suffix. Use
/// [`CpalSink::playback_progress`] to learn when accepted samples have been
/// consumed by the device callback and are predicted to have finished playing.
pub struct CpalSink {
    producer: AudioRingProducer,
    sample_rate: u32,
    stream: Option<SendStream>,
    /// Progress published by the current stream's output callback; replaced
    /// on every `start()` so counts never carry across streams.
    progress: Option<Arc<ProgressShared>>,
    /// Outstanding `discard_buffered` request on the current stream.
    flush_request: Option<u64>,
    /// Cumulative flushed count already reported on the current stream.
    flushed_reported: u64,
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
            progress: None,
            flush_request: None,
            flushed_reported: 0,
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
            progress: None,
            flush_request: None,
            flushed_reported: 0,
            device,
            config,
        })
    }

    /// Return a coherent progress snapshot without blocking for playback.
    /// Fails when the stream is not running, has failed, or its playback
    /// timing cannot be established. start() begins a new count at zero.
    ///
    /// A caller that has had `target` samples accepted by `write` on this
    /// started stream knows they have played once
    /// `consumed_samples >= target && remaining.is_zero()`
    /// ([`PlaybackProgress::has_played`]).
    pub fn playback_progress(&self) -> Result<PlaybackProgress> {
        running_progress(self.stream.is_some(), self.progress.as_deref())?
            .progress_at(Instant::now())
    }
}

/// Whether an output error callback ends the stream. CPAL reports an xrun
/// through the error callback and then recovers the stream itself, so an
/// underrun is not terminal; every other error fails the stream.
fn is_terminal_output_error(kind: cpal::ErrorKind) -> bool {
    kind != cpal::ErrorKind::Xrun
}

/// The current stream's progress state, or an error when no stream is
/// running (before `start`, after `stop`).
fn running_progress(
    stream_running: bool,
    progress: Option<&ProgressShared>,
) -> Result<&ProgressShared> {
    match progress {
        Some(progress) if stream_running => Ok(progress),
        _ => Err(anyhow!("output stream is not running")),
    }
}

/// Nonblocking, capped write into a running stream's TX ring: fails for a
/// stopped, failed, or callback-less stream, and otherwise accepts at most
/// the ring's free space so the caller retains the rest.
fn write_capped(
    producer: &mut AudioRingProducer,
    stream_running: bool,
    progress: Option<&ProgressShared>,
    samples: &[f32],
) -> Result<usize> {
    let progress = running_progress(stream_running, progress)?;
    if progress.is_failed() {
        return Err(anyhow!("output stream failed"));
    }
    if producer.is_abandoned() {
        return Err(anyhow!("output stream callback is gone"));
    }
    // Offer only what fits, so retained backpressure is a short count for
    // the caller to retry, not a drop counted as overflow.
    let take = samples.len().min(producer.available());
    Ok(producer.write(&samples[..take]))
}

impl AudioSink for CpalSink {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        write_capped(
            &mut self.producer,
            self.stream.is_some(),
            self.progress.as_deref(),
            samples,
        )
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        // Drop any previous stream first so its callback can no longer
        // publish into a progress state a caller might still read.
        self.stop()?;
        let (producer, mut consumer) = audio_ring(8192);
        let progress = Arc::new(ProgressShared::new(Instant::now()));
        let callback_progress = progress.clone();
        let error_progress = progress.clone();
        let mut callback_state = CallbackState::new(self.sample_rate);

        let stream = self
            .device
            .build_output_stream(
                self.config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let entry = Instant::now();
                    let ts = info.timestamp();
                    let delay = ts.playback.checked_duration_since(ts.callback);
                    fill_output(
                        &mut consumer,
                        data,
                        &callback_progress,
                        &mut callback_state,
                        entry,
                        delay,
                    );
                },
                move |err: cpal::Error| {
                    if is_terminal_output_error(err.kind()) {
                        error_progress.fail();
                        eprintln!("CPAL output error: {}", err);
                    } else {
                        eprintln!("CPAL output underrun (stream recovered): {}", err);
                    }
                },
                None,
            )
            .map_err(|e| anyhow!("Failed to build output stream: {}", e))?;

        stream
            .play()
            .map_err(|e| anyhow!("Failed to start output stream: {}", e))?;
        self.producer = producer;
        self.progress = Some(progress);
        self.flush_request = None;
        self.flushed_reported = 0;
        self.stream = Some(SendStream::new(stream));
        Ok(())
    }

    /// The TX ring's consumer lives in the output callback, so the drop is
    /// served there on its next run (see `fill_output`). A stopped or failed
    /// stream will never play what its ring holds: nothing to wait for.
    fn discard_buffered(&mut self) -> Result<Option<u64>> {
        let progress = match self.progress.as_deref() {
            Some(progress) if self.stream.is_some() && !progress.is_failed() => progress,
            _ => {
                self.flush_request = None;
                return Ok(Some(0));
            }
        };
        let request = *self
            .flush_request
            .get_or_insert_with(|| progress.request_flush());
        Ok(progress.flush_result(request).map(|total| {
            self.flush_request = None;
            let dropped = total - self.flushed_reported;
            self.flushed_reported = total;
            dropped
        }))
    }

    fn stop(&mut self) -> Result<()> {
        self.progress = None;
        self.flush_request = None;
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

    /// Before start / after stop there is no running stream: writes and
    /// progress both fail instead of feeding a disconnected ring.
    #[test]
    fn not_running_stream_rejects_writes_and_progress() {
        let (mut producer, _consumer) = audio_ring(64);
        assert!(write_capped(&mut producer, false, None, &[0.1; 4]).is_err());
        let progress = ProgressShared::new(Instant::now());
        assert!(write_capped(&mut producer, false, Some(&progress), &[0.1; 4]).is_err());
        assert!(running_progress(false, Some(&progress)).is_err());
        assert!(running_progress(true, None).is_err());
        assert_eq!(producer.overflow_count(), 0);
    }

    /// A running stream accepts at most its free space (a short count, not
    /// overflow); a failed stream or a dropped callback consumer fails.
    #[test]
    fn running_stream_caps_writes_and_fails_when_broken() {
        let (mut producer, consumer) = audio_ring(8);
        let progress = ProgressShared::new(Instant::now());
        assert_eq!(
            write_capped(&mut producer, true, Some(&progress), &[0.1; 20]).unwrap(),
            8
        );
        assert_eq!(
            write_capped(&mut producer, true, Some(&progress), &[0.1; 20]).unwrap(),
            0
        );
        assert_eq!(producer.overflow_count(), 0);
        progress.fail();
        assert!(write_capped(&mut producer, true, Some(&progress), &[0.1; 1]).is_err());
        let fresh = ProgressShared::new(Instant::now());
        assert_eq!(
            running_progress(true, Some(&fresh))
                .unwrap()
                .progress_at(Instant::now())
                .unwrap()
                .consumed_samples,
            0,
            "a new start's progress begins at zero"
        );
        drop(consumer);
        assert!(write_capped(&mut producer, true, Some(&fresh), &[0.1; 1]).is_err());
    }

    #[test]
    fn underruns_are_recoverable_other_errors_terminal() {
        assert!(!is_terminal_output_error(cpal::ErrorKind::Xrun));
        assert!(is_terminal_output_error(
            cpal::ErrorKind::DeviceNotAvailable
        ));
        assert!(is_terminal_output_error(cpal::ErrorKind::DeviceChanged));
    }

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
