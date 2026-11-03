//! File-based audio backends for regression testing.

use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};
use std::path::Path;

use crate::{AudioSink, AudioSource};

/// Read audio samples from a WAV file.
pub struct WavSource {
    samples: Vec<f32>,
    position: usize,
    sample_rate: u32,
}

/// Sample encoding of a file `WavSink` writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WavFormat {
    /// 16-bit signed PCM, the format every audio tool reads (COP-86).
    #[default]
    Pcm16,
    /// 32-bit IEEE float (`WAVE_FORMAT_EXTENSIBLE`), lossless for `f32`.
    Float32,
}

/// Write audio samples to a mono WAV file, 16-bit PCM by default.
///
/// The file is written once, by `stop()`. A sink dropped without `stop()`
/// still writes its samples, and logs a failure, as a safety net.
pub struct WavSink {
    samples: Vec<f32>,
    sample_rate: u32,
    path: std::path::PathBuf,
    format: WavFormat,
    /// Samples were written since the last `stop()`, so `Drop` must flush.
    pending: bool,
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
            .map_err(|e| anyhow!("Failed to open WAV file {}: {}", path.as_ref().display(), e))?;
        let spec = reader.spec();
        let sample_rate = spec.sample_rate;

        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .into_samples::<f32>()
                .enumerate()
                .map(|(index, sample)| {
                    sample.with_context(|| {
                        format!("reading WAV {} sample {index}", path.as_ref().display())
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            hound::SampleFormat::Int => {
                let max_val = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .enumerate()
                    .map(|(index, sample)| {
                        sample
                            .with_context(|| {
                                format!("reading WAV {} sample {index}", path.as_ref().display())
                            })
                            .map(|value| value as f32 / max_val)
                    })
                    .collect::<Result<Vec<_>>>()?
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
    /// Create a WAV sink that will write 16-bit PCM to the given path.
    pub fn new<P: AsRef<Path>>(path: P, sample_rate: u32) -> Self {
        Self::with_format(path, sample_rate, WavFormat::default())
    }

    /// Create a WAV sink that will write `format` samples to the given path.
    pub fn with_format<P: AsRef<Path>>(path: P, sample_rate: u32, format: WavFormat) -> Self {
        Self {
            samples: Vec::new(),
            sample_rate,
            path: path.as_ref().to_path_buf(),
            format,
            pending: false,
        }
    }

    /// Flush buffered samples to the WAV file.
    pub fn flush_to_file(&self) -> Result<()> {
        let (bits_per_sample, sample_format) = match self.format {
            WavFormat::Pcm16 => (16, hound::SampleFormat::Int),
            WavFormat::Float32 => (32, hound::SampleFormat::Float),
        };
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample,
            sample_format,
        };

        let mut writer = hound::WavWriter::create(&self.path, spec)
            .map_err(|e| anyhow!("Failed to create WAV file: {}", e))?;

        for &sample in &self.samples {
            let written = match self.format {
                WavFormat::Pcm16 => writer.write_sample(pcm16(sample)),
                WavFormat::Float32 => writer.write_sample(sample),
            };
            written.map_err(|e| anyhow!("Failed to write WAV sample: {}", e))?;
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
        if self.pending && !self.samples.is_empty() {
            if let Err(e) = self.flush_to_file() {
                tracing::error!(
                    path = %self.path.display(),
                    error = %e,
                    "WavSink dropped with unflushed samples and could not write them"
                );
            }
        }
    }
}

impl AudioSink for WavSink {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        self.samples.extend_from_slice(samples);
        self.pending = true;
        Ok(samples.len())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        // Cleared first: a failure is the caller's to report, once, and is
        // not retried (and logged again) by `Drop` (COP-86).
        self.pending = false;
        self.flush_to_file()
    }
}

/// One `f32` sample as 16-bit PCM, clamped to full scale. The 32768 scale
/// is the inverse of `WavSource`'s and `RawPcmReader`'s, so a round trip is
/// off by at most half a step.
fn pcm16(sample: f32) -> i16 {
    (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

/// Sample encoding of headerless PCM on a stream: mono, little-endian.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmFormat {
    /// 16-bit signed integer (`arecord -f S16_LE`, `ffmpeg -f s16le`).
    S16Le,
    /// 32-bit IEEE float (`ffmpeg -f f32le`).
    F32Le,
}

impl PcmFormat {
    /// Bytes in one sample.
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::S16Le => 2,
            Self::F32Le => 4,
        }
    }

    /// The WAV format holding the same samples.
    pub fn wav_format(self) -> WavFormat {
        match self {
            Self::S16Le => WavFormat::Pcm16,
            Self::F32Le => WavFormat::Float32,
        }
    }
}

impl std::str::FromStr for PcmFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "s16le" => Ok(Self::S16Le),
            "f32le" => Ok(Self::F32Le),
            other => Err(format!(
                "unknown sample format {other:?}: expected s16le or f32le"
            )),
        }
    }
}

impl std::fmt::Display for PcmFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::S16Le => "s16le",
            Self::F32Le => "f32le",
        })
    }
}

/// Read headerless mono PCM from a stream (`coppa rx -`), as it arrives.
///
/// `read` blocks until at least one whole sample is available, and returns
/// 0 only at end of stream ([`RawPcmReader::is_eof`]).
pub struct RawPcmReader<R: Read + Send> {
    inner: R,
    format: PcmFormat,
    sample_rate: u32,
    /// Bytes read but not yet returned as samples (a partial sample, or a
    /// peeked header).
    carry: Vec<u8>,
    eof: bool,
    trailing: usize,
}

impl<R: Read + Send> RawPcmReader<R> {
    /// Wrap `inner`, which carries `format` samples at `sample_rate`.
    pub fn new(inner: R, format: PcmFormat, sample_rate: u32) -> Self {
        Self {
            inner,
            format,
            sample_rate,
            carry: Vec::new(),
            eof: false,
            trailing: 0,
        }
    }

    /// The stream has ended.
    pub fn is_eof(&self) -> bool {
        self.eof
    }

    /// Bytes of an incomplete last sample that were dropped at end of stream.
    pub fn trailing_bytes(&self) -> usize {
        self.trailing
    }

    /// Whether the stream starts with a RIFF (WAV) header. The peeked bytes
    /// stay buffered, so nothing is lost when it does not.
    pub fn starts_with_wav_header(&mut self) -> Result<bool> {
        while self.carry.len() < 4 && !self.eof {
            self.fill()?;
        }
        Ok(self.carry.starts_with(b"RIFF"))
    }

    /// Read once from `inner` into `carry`, retrying `Interrupted`.
    fn fill(&mut self) -> Result<()> {
        let mut chunk = [0u8; 8192];
        loop {
            match self.inner.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(());
                }
                Ok(n) => {
                    self.carry.extend_from_slice(&chunk[..n]);
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(anyhow::Error::new(e).context("Failed to read raw PCM")),
            }
        }
    }

    fn decode_into(&mut self, buf: &mut [f32]) -> usize {
        let width = self.format.bytes_per_sample();
        let n = (self.carry.len() / width).min(buf.len());
        for (slot, bytes) in buf.iter_mut().zip(self.carry.chunks_exact(width)).take(n) {
            *slot = match self.format {
                PcmFormat::S16Le => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0,
                PcmFormat::F32Le => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            };
        }
        self.carry.drain(..n * width);
        n
    }
}

impl<R: Read + Send> AudioSource for RawPcmReader<R> {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let width = self.format.bytes_per_sample();
        while self.carry.len() < width && !self.eof {
            self.fill()?;
        }
        if self.carry.len() < width {
            let n = self.carry.len();
            if n > 0 && self.trailing == 0 {
                self.trailing = n;
                tracing::warn!(
                    bytes = n,
                    "raw PCM input ended mid-sample; ignored the last {n} byte(s)"
                );
            }
            self.carry.clear();
            return Ok(0);
        }
        Ok(self.decode_into(buf))
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

/// Write headerless mono PCM to a stream (`coppa tx -o -`) as it is
/// produced, with no whole-file buffer.
pub struct RawPcmWriter<W: Write + Send> {
    inner: W,
    format: PcmFormat,
    sample_rate: u32,
}

impl<W: Write + Send> RawPcmWriter<W> {
    /// Wrap `inner`, writing `format` samples at `sample_rate`.
    pub fn new(inner: W, format: PcmFormat, sample_rate: u32) -> Self {
        Self {
            inner,
            format,
            sample_rate,
        }
    }

    /// The wrapped writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write + Send> AudioSink for RawPcmWriter<W> {
    fn write(&mut self, samples: &[f32]) -> Result<usize> {
        let mut bytes = Vec::with_capacity(samples.len() * self.format.bytes_per_sample());
        for &sample in samples {
            match self.format {
                PcmFormat::S16Le => bytes.extend_from_slice(&pcm16(sample).to_le_bytes()),
                PcmFormat::F32Le => bytes.extend_from_slice(&sample.to_le_bytes()),
            }
        }
        // `.context` keeps the io::Error as the source, so a caller can see
        // a closed pipe (BrokenPipe) for what it is.
        self.inner
            .write_all(&bytes)
            .context("Failed to write raw PCM")?;
        Ok(samples.len())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.inner.flush().context("Failed to flush raw PCM")
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
    fn truncated_wav_samples_are_errors_for_integer_and_float() {
        for format in [hound::SampleFormat::Int, hound::SampleFormat::Float] {
            let path = std::env::temp_dir().join(format!(
                "coppa_truncated_{}_{}.wav",
                std::process::id(),
                if format == hound::SampleFormat::Int {
                    "int"
                } else {
                    "float"
                }
            ));
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: format,
            };
            let mut writer = hound::WavWriter::create(&path, spec).unwrap();
            if format == hound::SampleFormat::Int {
                writer.write_sample(42i32).unwrap();
            } else {
                writer.write_sample(0.5f32).unwrap();
            }
            writer.finalize().unwrap();
            assert_eq!(WavSource::open(&path).ok().unwrap().total_samples(), 1);
            let len = std::fs::metadata(&path).unwrap().len();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(len - 2)
                .unwrap();
            let err = WavSource::open(&path)
                .err()
                .expect("truncated sample must fail");
            assert!(err.to_string().contains("reading WAV"), "{err}");
            std::fs::remove_file(path).unwrap();
        }
    }

    /// A per-test temp path, removed when dropped.
    struct TempPath(std::path::PathBuf);

    impl TempPath {
        fn new(name: &str) -> Self {
            Self(std::env::temp_dir().join(format!("coppa_cop86_{}_{name}", std::process::id())))
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn write_wav(path: &Path, format: WavFormat, samples: &[f32]) {
        let mut sink = WavSink::with_format(path, 48000, format);
        sink.start().unwrap();
        sink.write(samples).unwrap();
        sink.stop().unwrap();
    }

    fn read_all(path: &Path) -> Vec<f32> {
        let mut source = WavSource::open(path).unwrap();
        let mut out = vec![0.0f32; source.total_samples()];
        let n = source.read(&mut out).unwrap();
        assert_eq!(n, out.len());
        out
    }

    #[test]
    fn wav_sink_defaults_to_16_bit_pcm() {
        let path = TempPath::new("default16.wav");
        let mut sink = WavSink::new(&path.0, 48000);
        sink.start().unwrap();
        sink.write(&[0.0, 0.5, -0.5]).unwrap();
        sink.stop().unwrap();
        let spec = hound::WavReader::open(&path.0).unwrap().spec();
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48000);
    }

    #[test]
    fn wav_sink_pcm16_round_trip() {
        let path = TempPath::new("pcm16.wav");
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.001).sin()).collect();
        write_wav(&path.0, WavFormat::Pcm16, &input);
        let output = read_all(&path.0);
        for (i, (a, b)) in input.iter().zip(&output).enumerate() {
            assert!((a - b).abs() <= 1.0 / 32767.0, "sample {i}: {a} vs {b}");
        }

        let clipped = TempPath::new("pcm16_clip.wav");
        write_wav(&clipped.0, WavFormat::Pcm16, &[2.0, -2.0]);
        let output = read_all(&clipped.0);
        assert!((output[0] - 1.0).abs() <= 1.0 / 32767.0, "{output:?}");
        assert!((output[1] + 1.0).abs() <= 1.0 / 32767.0, "{output:?}");
    }

    #[test]
    fn wav_sink_writes_once() {
        let path = TempPath::new("once.wav");
        let mut sink = WavSink::new(&path.0, 48000);
        sink.write(&[0.25; 100]).unwrap();
        sink.stop().unwrap();
        std::fs::remove_file(&path.0).unwrap();
        drop(sink);
        assert!(!path.0.exists(), "Drop wrote the file a second time");
    }

    #[test]
    fn failed_stop_is_not_retried_on_drop() {
        let path = std::env::temp_dir()
            .join(format!("coppa_cop86_missing_{}", std::process::id()))
            .join("x.wav");
        let mut sink = WavSink::new(&path, 48000);
        sink.write(&[0.25; 100]).unwrap();
        assert!(sink.pending);
        assert!(sink.stop().is_err());
        assert!(!sink.pending, "Drop would retry and log the failure again");
    }

    #[test]
    fn dropped_unstopped_sink_still_flushes() {
        let path = TempPath::new("dropped.wav");
        let mut sink = WavSink::new(&path.0, 48000);
        sink.write(&[0.25; 100]).unwrap();
        drop(sink);
        assert_eq!(read_all(&path.0).len(), 100);
    }

    /// A reader that hands out one byte per `read`, splitting samples.
    struct OneByte(std::io::Cursor<Vec<u8>>);

    impl Read for OneByte {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let end = buf.len().min(1);
            self.0.read(&mut buf[..end])
        }
    }

    fn read_to_end<R: Read + Send>(reader: &mut RawPcmReader<R>) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = [0.0f32; 3];
        loop {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 {
                assert!(reader.is_eof());
                return out;
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn raw_pcm_reader_s16le_and_f32le() {
        let values = [0i16, 16384, -16384, 32767, -32768];
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let expected: Vec<f32> = values.iter().map(|&v| f32::from(v) / 32768.0).collect();
        let mut whole =
            RawPcmReader::new(std::io::Cursor::new(bytes.clone()), PcmFormat::S16Le, 48000);
        assert_eq!(read_to_end(&mut whole), expected);
        let mut split = RawPcmReader::new(
            OneByte(std::io::Cursor::new(bytes)),
            PcmFormat::S16Le,
            48000,
        );
        assert_eq!(read_to_end(&mut split), expected);

        let floats = [0.0f32, 0.123, -0.75, 1.0];
        let bytes: Vec<u8> = floats.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut reader = RawPcmReader::new(
            OneByte(std::io::Cursor::new(bytes)),
            PcmFormat::F32Le,
            48000,
        );
        assert_eq!(read_to_end(&mut reader), floats);
        assert_eq!(reader.trailing_bytes(), 0);
    }

    #[test]
    fn raw_pcm_reader_trailing_partial_sample() {
        let mut reader = RawPcmReader::new(
            std::io::Cursor::new(b"abc".to_vec()),
            PcmFormat::S16Le,
            48000,
        );
        let samples = read_to_end(&mut reader);
        assert_eq!(samples.len(), 1);
        assert_eq!(reader.trailing_bytes(), 1);
    }

    #[test]
    fn raw_pcm_reader_peeks_a_wav_header_without_losing_bytes() {
        let mut wav = RawPcmReader::new(
            std::io::Cursor::new(b"RIFF....".to_vec()),
            PcmFormat::S16Le,
            48000,
        );
        assert!(wav.starts_with_wav_header().unwrap());

        let bytes: Vec<u8> = [100i16, -100, 200]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut raw = RawPcmReader::new(
            OneByte(std::io::Cursor::new(bytes)),
            PcmFormat::S16Le,
            48000,
        );
        assert!(!raw.starts_with_wav_header().unwrap());
        assert_eq!(read_to_end(&mut raw).len(), 3);

        let mut short = RawPcmReader::new(
            std::io::Cursor::new(b"RI".to_vec()),
            PcmFormat::S16Le,
            48000,
        );
        assert!(!short.starts_with_wav_header().unwrap());
    }

    #[test]
    fn raw_pcm_writer_s16le_and_f32le() {
        let samples = [0.0f32, 0.5, -0.5, 2.0];
        let mut writer = RawPcmWriter::new(Vec::new(), PcmFormat::S16Le, 48000);
        assert_eq!(writer.write(&samples).unwrap(), 4);
        writer.stop().unwrap();
        let expected: Vec<u8> = [0i16, 16384, -16384, 32767]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(writer.into_inner(), expected);

        let mut writer = RawPcmWriter::new(Vec::new(), PcmFormat::F32Le, 48000);
        writer.write(&samples).unwrap();
        let expected: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(writer.into_inner(), expected);
    }

    #[test]
    fn pcm_format_parses() {
        assert_eq!("s16le".parse::<PcmFormat>(), Ok(PcmFormat::S16Le));
        assert_eq!("f32le".parse::<PcmFormat>(), Ok(PcmFormat::F32Le));
        for bad in ["S16LE", "s16", "", "wav"] {
            assert!(bad.parse::<PcmFormat>().is_err(), "{bad:?}");
        }
        assert_eq!(PcmFormat::S16Le.to_string(), "s16le");
        assert_eq!(PcmFormat::F32Le.bytes_per_sample(), 4);
    }

    #[test]
    fn wav_sink_float_format_round_trips_exactly() {
        let dir = std::env::temp_dir();
        let path = dir.join("coppa_test_roundtrip.wav");

        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.001).sin()).collect();

        // Write
        let mut sink = WavSink::with_format(&path, 48000, WavFormat::Float32);
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
