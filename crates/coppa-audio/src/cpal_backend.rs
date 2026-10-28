//! CPAL audio backend for real hardware I/O.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use std::str::FromStr;

use std::sync::Arc;
use std::time::Instant;

use crate::playout::{fill_output, CallbackState, PlaybackProgress, ProgressShared};
use crate::ringbuf::{audio_ring, AudioRingConsumer, AudioRingProducer};
use crate::{AudioDevice, AudioSink, AudioSource};

/// A device's own name and cpal id as the host reports them, e.g.
/// `Default Audio Device (alsa:default)`; either part is `unknown` when the
/// host cannot say.
fn name_of(device: &cpal::Device) -> String {
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let id = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    format!("{name} ({id})")
}

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
            id: device.id().ok().map(|id| id.to_string()),
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
    channel: u16,
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
    channel: u16,
}

impl CpalSource {
    /// The opened device's name and cpal id (see `name_of`), also for the
    /// default device, e.g. `Default Audio Device (alsa:default)`.
    pub fn device_name(&self) -> String {
        name_of(&self.device)
    }

    /// Create a new CPAL source using the default input device.
    ///
    /// You must call `start()` before `read()` will return any data.
    pub fn new(sample_rate: u32, _buffer_size: usize) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("No input device available"))?;

        let config =
            choose_device_config(&device, AudioDirection::Input, sample_rate, sample_rate, 0)?;

        // Create a placeholder consumer; start() will replace it with a
        // properly wired ring buffer connected to the CPAL callback.
        let (_producer, consumer) = audio_ring(64);

        Ok(Self {
            consumer,
            sample_rate,
            stream: None,
            device,
            config,
            channel: 0,
        })
    }

    /// Create a CPAL source from a specific device.
    pub fn from_device(
        device: cpal::Device,
        sample_rate: u32,
        _buffer_size: usize,
    ) -> Result<Self> {
        let config =
            choose_device_config(&device, AudioDirection::Input, sample_rate, sample_rate, 0)?;

        let (_producer, consumer) = audio_ring(64);

        Ok(Self {
            consumer,
            sample_rate,
            stream: None,
            device,
            config,
            channel: 0,
        })
    }

    pub fn with_options(
        device: Option<cpal::Device>,
        engine_rate: u32,
        preferred_rate: u32,
        buffer_size: usize,
        channel: u16,
    ) -> Result<Self> {
        let _ = buffer_size;
        let device = match device {
            Some(d) => d,
            None => cpal::default_host()
                .default_input_device()
                .ok_or_else(|| anyhow!("No input device available"))?,
        };
        let config = choose_device_config(
            &device,
            AudioDirection::Input,
            engine_rate,
            preferred_rate,
            channel,
        )?;
        let (_producer, consumer) = audio_ring(64);
        Ok(Self {
            consumer,
            sample_rate: config.sample_rate,
            stream: None,
            device,
            config,
            channel,
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

        let channels = self.config.channels as usize;
        let channel = self.channel as usize;
        let stream = self
            .device
            .build_input_stream(
                self.config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    for frames in data.chunks(1024 * channels) {
                        let mut mono = [0.0f32; 1024];
                        let n = extract_lane(frames, channels, channel, &mut mono);
                        producer.write(&mono[..n]);
                    }
                },
                |err| {
                    tracing::error!(error = %err, "CPAL input error");
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
    /// The opened device's name and cpal id (see `name_of`), also for the
    /// default device, e.g. `Default Audio Device (alsa:default)`.
    pub fn device_name(&self) -> String {
        name_of(&self.device)
    }

    /// Create a new CPAL sink using the default output device.
    ///
    /// You must call `start()` before `write()` will output audio.
    pub fn new(sample_rate: u32, _buffer_size: usize) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No output device available"))?;

        let config =
            choose_device_config(&device, AudioDirection::Output, sample_rate, sample_rate, 0)?;

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
            channel: 0,
        })
    }

    /// Create a CPAL sink from a specific device.
    pub fn from_device(
        device: cpal::Device,
        sample_rate: u32,
        _buffer_size: usize,
    ) -> Result<Self> {
        let config =
            choose_device_config(&device, AudioDirection::Output, sample_rate, sample_rate, 0)?;

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
            channel: 0,
        })
    }

    pub fn with_options(
        device: Option<cpal::Device>,
        engine_rate: u32,
        preferred_rate: u32,
        buffer_size: usize,
        channel: u16,
    ) -> Result<Self> {
        let _ = buffer_size;
        let device = match device {
            Some(d) => d,
            None => cpal::default_host()
                .default_output_device()
                .ok_or_else(|| anyhow!("No output device available"))?,
        };
        let config = choose_device_config(
            &device,
            AudioDirection::Output,
            engine_rate,
            preferred_rate,
            channel,
        )?;
        let (producer, _consumer) = audio_ring(64);
        Ok(Self {
            producer,
            sample_rate: config.sample_rate,
            stream: None,
            progress: None,
            flush_request: None,
            flushed_reported: 0,
            device,
            config,
            channel,
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
        let callback_rate = self.sample_rate;
        let channels = self.config.channels as usize;
        let channel = self.channel as usize;

        let stream = self
            .device
            .build_output_stream(
                self.config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let entry = Instant::now();
                    let ts = info.timestamp();
                    let delay = ts.playback.checked_duration_since(ts.callback);
                    for (chunk_index, frames) in data.chunks_mut(1024 * channels).enumerate() {
                        let mut mono = [0.0f32; 1024];
                        let count = frames.len() / channels;
                        let offset = std::time::Duration::from_secs_f64(
                            (chunk_index * 1024) as f64 / callback_rate as f64,
                        );
                        fill_output(
                            &mut consumer,
                            &mut mono[..count],
                            &callback_progress,
                            &mut callback_state,
                            entry + offset,
                            delay,
                        );
                        route_lane(&mono[..count], frames, channels, channel);
                    }
                },
                move |err: cpal::Error| {
                    if is_terminal_output_error(err.kind()) {
                        error_progress.fail();
                        tracing::error!(error = %err, "CPAL output error");
                    } else {
                        tracing::warn!(error = %err, "CPAL output underrun (stream recovered)");
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioDirection {
    Input,
    Output,
}

impl AudioDirection {
    fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
    fn available(self, d: &AudioDevice) -> bool {
        match self {
            Self::Input => d.input_channels > 0,
            Self::Output => d.output_channels > 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelectionError {
    MissingName {
        direction: &'static str,
        name: String,
    },
    AmbiguousName {
        direction: &'static str,
        name: String,
        candidates: Vec<String>,
    },
    MissingId {
        direction: &'static str,
        id: String,
    },
    WrongDirection {
        direction: &'static str,
        id: String,
    },
}

impl std::fmt::Display for DeviceSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingName { direction, name } => write!(f, "{direction} audio device name {name:?} not found; run `coppa devices` to list available devices"),
            Self::AmbiguousName { direction, name, candidates } => write!(f, "{direction} device name {name:?} matches {} devices: {}.", candidates.len(), candidates.join("; ")),
            Self::MissingId { direction, id } => write!(f, "{direction} audio device ID {id:?} not found; run `coppa devices` to list available IDs"),
            Self::WrongDirection { direction, id } => write!(f, "{direction} audio device ID {id:?} has no {direction} channels; run `coppa devices` to check directions"),
        }
    }
}
impl std::error::Error for DeviceSelectionError {}

fn candidate_label(d: &AudioDevice) -> String {
    format!(
        "{} [id: {}]",
        d.name,
        d.id.as_deref().unwrap_or("unavailable")
    )
}

fn resolve_candidate(
    inventory: &[AudioDevice],
    direction: AudioDirection,
    name: &str,
) -> std::result::Result<usize, DeviceSelectionError> {
    let needle = name.to_lowercase();
    let matches: Vec<usize> = inventory
        .iter()
        .enumerate()
        .filter(|(_, d)| direction.available(d) && d.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(DeviceSelectionError::MissingName {
            direction: direction.label(),
            name: name.to_string(),
        }),
        _ => Err(DeviceSelectionError::AmbiguousName {
            direction: direction.label(),
            name: name.to_string(),
            candidates: matches
                .iter()
                .map(|i| candidate_label(&inventory[*i]))
                .collect(),
        }),
    }
}

fn resolve_id_candidate(
    inventory: &[AudioDevice],
    direction: AudioDirection,
    id: &str,
) -> std::result::Result<usize, DeviceSelectionError> {
    let Some(index) = inventory.iter().position(|d| d.id.as_deref() == Some(id)) else {
        return Err(DeviceSelectionError::MissingId {
            direction: direction.label(),
            id: id.to_string(),
        });
    };
    if !direction.available(&inventory[index]) {
        return Err(DeviceSelectionError::WrongDirection {
            direction: direction.label(),
            id: id.to_string(),
        });
    }
    Ok(index)
}

fn device_capable(device: &cpal::Device, direction: AudioDirection) -> bool {
    match direction {
        AudioDirection::Input => device
            .supported_input_configs()
            .map(|mut c| c.any(|r| r.channels() > 0))
            .unwrap_or(false),
        AudioDirection::Output => device
            .supported_output_configs()
            .map(|mut c| c.any(|r| r.channels() > 0))
            .unwrap_or(false),
    }
}

fn resolve_device(
    name: Option<&str>,
    id: Option<&str>,
    direction: AudioDirection,
) -> Result<Option<cpal::Device>> {
    let host = cpal::default_host();
    if let Some(id) = id {
        let parsed = cpal::DeviceId::from_str(id).map_err(|_| DeviceSelectionError::MissingId {
            direction: direction.label(),
            id: id.to_string(),
        })?;
        let device = host
            .device_by_id(&parsed)
            .ok_or_else(|| DeviceSelectionError::MissingId {
                direction: direction.label(),
                id: id.to_string(),
            })?;
        let candidate = AudioDevice {
            name: device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| "Unknown".into()),
            id: Some(parsed.to_string()),
            max_sample_rate: 0,
            input_channels: if device_capable(&device, AudioDirection::Input) {
                1
            } else {
                0
            },
            output_channels: if device_capable(&device, AudioDirection::Output) {
                1
            } else {
                0
            },
        };
        resolve_id_candidate(&[candidate], direction, &parsed.to_string())?;
        return Ok(Some(device));
    }
    let Some(name) = name.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let devices: Vec<_> = host.devices()?.collect();
    let inventory: Vec<_> = devices
        .iter()
        .map(|d| AudioDevice {
            name: d
                .description()
                .map(|x| x.name().to_string())
                .unwrap_or_else(|_| "Unknown".into()),
            id: d.id().ok().map(|x| x.to_string()),
            max_sample_rate: 0,
            input_channels: if device_capable(d, AudioDirection::Input) {
                1
            } else {
                0
            },
            output_channels: if device_capable(d, AudioDirection::Output) {
                1
            } else {
                0
            },
        })
        .collect();
    let index = resolve_candidate(&inventory, direction, name)?;
    Ok(Some(devices[index].clone()))
}

pub fn resolve_input_device(name: Option<&str>, id: Option<&str>) -> Result<Option<cpal::Device>> {
    resolve_device(name, id, AudioDirection::Input)
}
pub fn resolve_output_device(name: Option<&str>, id: Option<&str>) -> Result<Option<cpal::Device>> {
    resolve_device(name, id, AudioDirection::Output)
}

/// Legacy lookup. Ambiguity and errors now produce no device.
pub fn find_input_device_by_name(name: &str) -> Option<cpal::Device> {
    resolve_input_device(Some(name), None).ok().flatten()
}
/// Legacy lookup. Ambiguity and errors now produce no device.
pub fn find_output_device_by_name(name: &str) -> Option<cpal::Device> {
    resolve_output_device(Some(name), None).ok().flatten()
}

#[derive(Clone, Copy, Debug)]
struct FormatRange {
    channels: u16,
    min_rate: u32,
    max_rate: u32,
    f32_format: bool,
}

fn choose_format(
    ranges: &[FormatRange],
    direction: AudioDirection,
    engine_rate: u32,
    preferred_rate: u32,
    channel: u16,
) -> Result<(u16, u32)> {
    if engine_rate == 0 || preferred_rate == 0 {
        return Err(anyhow!("audio sample rate must be positive"));
    }
    let mut best: Option<(u8, u64, u16, u32)> = None;
    let mut max_channels = 0u16;
    for range in ranges {
        if !range.f32_format {
            continue;
        }
        max_channels = max_channels.max(range.channels);
        if channel >= range.channels || range.min_rate == 0 || range.min_rate > range.max_rate {
            continue;
        }
        let preferred_supported =
            range.min_rate <= preferred_rate && preferred_rate <= range.max_rate;
        let rate = if preferred_supported {
            preferred_rate
        } else {
            engine_rate.clamp(range.min_rate, range.max_rate)
        };
        let candidate = (
            if preferred_supported { 0 } else { 1 },
            rate.abs_diff(engine_rate) as u64,
            range.channels,
            rate,
        );
        if best.is_none_or(|b| candidate < b) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, channels, rate)| (channels, rate))
        .ok_or_else(|| {
            if channel >= max_channels && max_channels > 0 {
                anyhow!(
                    "{} channel {} is unavailable; available channels: 0..{}",
                    direction.label(),
                    channel,
                    max_channels - 1
                )
            } else {
                anyhow!("no usable f32 {} audio format", direction.label())
            }
        })
}

fn choose_device_config(
    device: &cpal::Device,
    direction: AudioDirection,
    engine_rate: u32,
    preferred_rate: u32,
    channel: u16,
) -> Result<StreamConfig> {
    let map_range = |c: cpal::SupportedStreamConfigRange| FormatRange {
        channels: c.channels(),
        min_rate: c.min_sample_rate(),
        max_rate: c.max_sample_rate(),
        f32_format: c.sample_format() == cpal::SampleFormat::F32,
    };
    let ranges: Vec<_> = match direction {
        AudioDirection::Input => device.supported_input_configs()?.map(map_range).collect(),
        AudioDirection::Output => device.supported_output_configs()?.map(map_range).collect(),
    };
    let max_channels = ranges
        .iter()
        .filter(|r| r.f32_format)
        .map(|r| r.channels)
        .max()
        .unwrap_or(0);
    if max_channels > 0 && channel >= max_channels {
        let id = device
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unavailable".into());
        let available = (0..max_channels)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow!(
            "{} channel {} is unavailable on device {:?}; available channels: {}",
            direction.label(),
            channel,
            id,
            available
        ));
    }
    let (channels, sample_rate) =
        choose_format(&ranges, direction, engine_rate, preferred_rate, channel)?;
    Ok(StreamConfig {
        channels,
        sample_rate,
        buffer_size: cpal::BufferSize::Default,
    })
}

fn extract_lane(interleaved: &[f32], channels: usize, channel: usize, mono: &mut [f32]) -> usize {
    let n = (interleaved.len() / channels).min(mono.len());
    for (dst, frame) in mono[..n].iter_mut().zip(interleaved.chunks_exact(channels)) {
        *dst = frame[channel];
    }
    n
}

fn route_lane(mono: &[f32], interleaved: &mut [f32], channels: usize, channel: usize) {
    interleaved.fill(0.0);
    for (sample, frame) in mono.iter().zip(interleaved.chunks_exact_mut(channels)) {
        frame[channel] = *sample;
    }
}

/// A live input stream with automatic hardware-to-engine rate conversion.
pub struct LiveSource {
    inner: LiveSourceInner,
    device_name: String,
    hardware_rate: u32,
}
enum LiveSourceInner {
    Direct(CpalSource),
    Resampled(Box<crate::resampler::ResamplingSource<CpalSource>>),
}
impl LiveSource {
    pub fn new(
        device: Option<cpal::Device>,
        engine_rate: u32,
        preferred_hw_rate: u32,
        buffer_size: usize,
        channel: u16,
    ) -> Result<Self> {
        let source =
            CpalSource::with_options(device, engine_rate, preferred_hw_rate, buffer_size, channel)?;
        let device_name = source.device_name();
        let hardware_rate = source.sample_rate();
        let inner = if hardware_rate == engine_rate {
            LiveSourceInner::Direct(source)
        } else {
            LiveSourceInner::Resampled(Box::new(crate::resampler::ResamplingSource::new(
                source,
                engine_rate,
            )?))
        };
        Ok(Self {
            inner,
            device_name,
            hardware_rate,
        })
    }
    pub fn device_name(&self) -> &str {
        &self.device_name
    }
    pub fn hardware_rate(&self) -> u32 {
        self.hardware_rate
    }
}
impl AudioSource for LiveSource {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        match &mut self.inner {
            LiveSourceInner::Direct(s) => s.read(buf),
            LiveSourceInner::Resampled(s) => s.read(buf),
        }
    }
    fn sample_rate(&self) -> u32 {
        match &self.inner {
            LiveSourceInner::Direct(s) => s.sample_rate(),
            LiveSourceInner::Resampled(s) => s.sample_rate(),
        }
    }
    fn start(&mut self) -> Result<()> {
        match &mut self.inner {
            LiveSourceInner::Direct(s) => s.start(),
            LiveSourceInner::Resampled(s) => s.start(),
        }
    }
    fn stop(&mut self) -> Result<()> {
        match &mut self.inner {
            LiveSourceInner::Direct(s) => s.stop(),
            LiveSourceInner::Resampled(s) => s.stop(),
        }
    }
}

/// A live output stream with bounded conversion and engine-unit progress.
pub struct LiveSink {
    inner: LiveSinkInner,
    device_name: String,
    hardware_rate: u32,
}
enum LiveSinkInner {
    Direct(Box<CpalSink>),
    Resampled(Box<crate::resampler::ResamplingSink<CpalSink>>),
}
impl LiveSink {
    pub fn new(
        device: Option<cpal::Device>,
        engine_rate: u32,
        preferred_hw_rate: u32,
        buffer_size: usize,
        channel: u16,
    ) -> Result<Self> {
        let sink =
            CpalSink::with_options(device, engine_rate, preferred_hw_rate, buffer_size, channel)?;
        let device_name = sink.device_name();
        let hardware_rate = sink.sample_rate();
        let inner = if hardware_rate == engine_rate {
            LiveSinkInner::Direct(Box::new(sink))
        } else {
            LiveSinkInner::Resampled(Box::new(crate::resampler::ResamplingSink::new(
                sink,
                engine_rate,
            )?))
        };
        Ok(Self {
            inner,
            device_name,
            hardware_rate,
        })
    }
    pub fn device_name(&self) -> &str {
        &self.device_name
    }
    pub fn hardware_rate(&self) -> u32 {
        self.hardware_rate
    }
    pub fn playback_progress(&self) -> Result<PlaybackProgress> {
        match &self.inner {
            LiveSinkInner::Direct(s) => s.playback_progress(),
            LiveSinkInner::Resampled(s) => {
                Ok(s.progress_from_hardware(s.inner().playback_progress()?))
            }
        }
    }
    pub fn finish_input(&mut self) -> Result<()> {
        AudioSink::finish_input(self)
    }
}
impl AudioSink for LiveSink {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        match &mut self.inner {
            LiveSinkInner::Direct(s) => s.write(samples),
            LiveSinkInner::Resampled(s) => s.write(samples),
        }
    }
    fn sample_rate(&self) -> u32 {
        match &self.inner {
            LiveSinkInner::Direct(s) => s.sample_rate(),
            LiveSinkInner::Resampled(s) => s.sample_rate(),
        }
    }
    fn start(&mut self) -> Result<()> {
        match &mut self.inner {
            LiveSinkInner::Direct(s) => s.start(),
            LiveSinkInner::Resampled(s) => s.start(),
        }
    }
    fn stop(&mut self) -> Result<()> {
        match &mut self.inner {
            LiveSinkInner::Direct(s) => s.stop(),
            LiveSinkInner::Resampled(s) => s.stop(),
        }
    }
    fn finish_input(&mut self) -> Result<()> {
        match &mut self.inner {
            LiveSinkInner::Direct(s) => s.finish_input(),
            LiveSinkInner::Resampled(s) => s.finish_input(),
        }
    }
    fn discard_buffered(&mut self) -> Result<Option<u64>> {
        match &mut self.inner {
            LiveSinkInner::Direct(s) => s.discard_buffered(),
            LiveSinkInner::Resampled(s) => s.discard_buffered(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, id: Option<&str>, input: u16, output: u16) -> AudioDevice {
        AudioDevice {
            name: name.into(),
            id: id.map(str::to_string),
            max_sample_rate: 48000,
            input_channels: input,
            output_channels: output,
        }
    }

    #[test]
    fn same_name_devices_are_ambiguous_in_stable_inventory_order() {
        let devices = vec![
            fixture("USB Audio", Some("alsa:radio-a"), 2, 2),
            fixture("USB Audio", Some("alsa:radio-b"), 2, 2),
        ];
        let err = resolve_candidate(&devices, AudioDirection::Input, "USB").unwrap_err();
        assert_eq!(
            err,
            DeviceSelectionError::AmbiguousName {
                direction: "input",
                name: "USB".into(),
                candidates: vec![
                    "USB Audio [id: alsa:radio-a]".into(),
                    "USB Audio [id: alsa:radio-b]".into()
                ]
            }
        );
    }

    #[test]
    fn ambiguous_name_error_is_usable_by_every_caller() {
        let error = DeviceSelectionError::AmbiguousName {
            direction: "input",
            name: "USB".into(),
            candidates: vec![
                "USB Audio [id: alsa:radio-a]".into(),
                "USB Audio [id: alsa:radio-b]".into(),
            ],
        };
        let message = error.to_string();
        assert!(message.contains("alsa:radio-a") && message.contains("alsa:radio-b"));
        assert!(!message.contains("--device-id"), "{message}");
    }

    #[test]
    fn resolver_filters_direction_and_handles_unavailable_ids() {
        let devices = vec![
            fixture("USB input", None, 2, 0),
            fixture("USB output", Some("alsa:out"), 0, 2),
        ];
        assert_eq!(
            resolve_candidate(&devices, AudioDirection::Input, "USB").unwrap(),
            0
        );
        assert_eq!(
            resolve_candidate(&devices, AudioDirection::Output, "USB").unwrap(),
            1
        );
        assert_eq!(candidate_label(&devices[0]), "USB input [id: unavailable]");
        assert!(matches!(
            resolve_candidate(&devices, AudioDirection::Input, "missing"),
            Err(DeviceSelectionError::MissingName { .. })
        ));
        assert_eq!(
            resolve_id_candidate(&devices, AudioDirection::Output, "alsa:out").unwrap(),
            1
        );
        assert!(matches!(
            resolve_id_candidate(&devices, AudioDirection::Input, "alsa:out"),
            Err(DeviceSelectionError::WrongDirection { .. })
        ));
        assert!(matches!(
            resolve_id_candidate(&devices, AudioDirection::Output, "alsa:stale"),
            Err(DeviceSelectionError::MissingId { .. })
        ));
        assert!(matches!(
            resolve_id_candidate(&devices, AudioDirection::Input, "unavailable"),
            Err(DeviceSelectionError::MissingId { .. })
        ));
        assert!(resolve_input_device(None, None).unwrap().is_none());
        assert!(resolve_output_device(None, None).unwrap().is_none());
        assert!(resolve_input_device(None, Some("")).is_err());
    }

    #[test]
    fn chooses_stereo_when_mono_is_unavailable_and_rejects_bad_lane() {
        let stereo = [FormatRange {
            channels: 2,
            min_rate: 44100,
            max_rate: 44100,
            f32_format: true,
        }];
        assert_eq!(
            choose_format(&stereo, AudioDirection::Input, 48000, 48000, 1).unwrap(),
            (2, 44100)
        );
        assert!(
            choose_format(&stereo, AudioDirection::Input, 48000, 48000, 2)
                .unwrap_err()
                .to_string()
                .contains("input channel 2")
        );
        let both = [
            FormatRange {
                channels: 2,
                min_rate: 48000,
                max_rate: 48000,
                f32_format: true,
            },
            FormatRange {
                channels: 1,
                min_rate: 48000,
                max_rate: 48000,
                f32_format: true,
            },
        ];
        assert_eq!(
            choose_format(&both, AudioDirection::Output, 48000, 48000, 0).unwrap(),
            (1, 48000)
        );
        assert!(choose_format(&both[1..], AudioDirection::Output, 48000, 48000, 1).is_err());
        assert!(choose_format(
            &[FormatRange {
                f32_format: false,
                ..both[0]
            }],
            AudioDirection::Output,
            48000,
            48000,
            0
        )
        .is_err());
    }

    #[test]
    fn capture_and_playback_route_only_the_selected_mono_lane() {
        let mut mono = [0.0; 2];
        assert_eq!(extract_lane(&[1.0, 2.0, 3.0, 4.0], 2, 0, &mut mono), 2);
        assert_eq!(mono, [1.0, 3.0]);
        assert_eq!(extract_lane(&[1.0, 2.0, 3.0, 4.0], 2, 1, &mut mono), 2);
        assert_eq!(mono, [2.0, 4.0]);
        let mut stereo = [9.0; 4];
        route_lane(&[0.25, 0.75], &mut stereo, 2, 0);
        assert_eq!(stereo, [0.25, 0.0, 0.75, 0.0]);
        route_lane(&[0.25, 0.75], &mut stereo, 2, 1);
        assert_eq!(stereo, [0.0, 0.25, 0.0, 0.75]);
        let (mut producer, mut consumer) = audio_ring(8);
        producer.write(&[0.25, 0.75]);
        let base = Instant::now();
        let shared = ProgressShared::new(base);
        let mut state = CallbackState::new(48000);
        let mut callback_mono = [0.0; 2];
        assert_eq!(
            fill_output(
                &mut consumer,
                &mut callback_mono,
                &shared,
                &mut state,
                base,
                Some(std::time::Duration::ZERO)
            ),
            2
        );
        route_lane(&callback_mono, &mut stereo, 2, 1);
        assert_eq!(shared.progress_at(base).unwrap().consumed_samples, 2);
    }

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
