//! coppa's AFSK 1200 TNC measured against Direwolf on the same recording (COP-112).
//!
//! Counts the AX.25 packets with a valid FCS that coppa's TNC receive chain decodes from a WAV, and
//! the packets Direwolf's `direwolf` and `atest` programs decode from the same file, then formats a
//! report with coppa's count as a percentage of each. COP-99's target is at least 95 percent of
//! Direwolf's count; this module only measures.
//!
//! The program is `crates/coppa-bench/diagnostics/afsk_vs_direwolf.rs`. `tools/tnc-bench/run.sh`
//! fetches WA8LMF TNC Test CD track 2, checks it against pinned SHA-256 sums and runs the program
//! on it. The logic lives here, not in the program, because CI compiles bench examples but never
//! runs them; these unit tests run in `cargo test --workspace --lib`.

use anyhow::{anyhow, bail, Context, Result};
use coppa_audio::resampler::ResamplingSource;
use coppa_audio::{AudioSource, WavSource};
use coppa_codec::afsk::Demodulator;
use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

/// The AFSK demodulator's fixed input rate (`coppa_codec::afsk`).
pub const ENGINE_RATE: u32 = 48_000;
/// Samples per read, as in the TNC's demod worker (`crates/coppa-daemon/src/tnc.rs`,
/// `run_demod_worker`).
pub const BLOCK: usize = 1024;
/// Start offsets in input samples for the alignment spread, all under one 1200-baud symbol at
/// 44.1 kHz (36.75 samples).
pub const START_OFFSETS: [usize; 8] = [0, 5, 10, 15, 20, 25, 30, 35];

/// Bytes per write when streaming samples into `direwolf`'s stdin.
const STDIN_CHUNK: usize = 64 * 1024;

// ---------------------------------------------------------------------------------------------
// coppa's side
// ---------------------------------------------------------------------------------------------

/// An `AudioSource` over borrowed samples, so start offsets need no copies.
struct SliceSource<'a> {
    samples: &'a [f32],
    pos: usize,
    rate: u32,
}

impl<'a> SliceSource<'a> {
    fn new(samples: &'a [f32], rate: u32) -> Self {
        Self {
            samples,
            pos: 0,
            rate,
        }
    }
}

impl AudioSource for SliceSource<'_> {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize> {
        let remaining = &self.samples[self.pos..];
        let n = buf.len().min(remaining.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        self.pos += n;
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

/// Feeds `src` to a fresh demodulator in `BLOCK`-sample reads until it returns 0, as
/// `run_demod_worker` in `crates/coppa-daemon/src/tnc.rs` does, and counts the frames.
fn drain_and_count<S: AudioSource>(mut src: S) -> Result<usize> {
    let mut demod = Demodulator::new();
    let mut buf = vec![0.0f32; BLOCK];
    let mut count = 0;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        demod.process(&buf[..n]);
        count += demod.take_frames().len();
    }
    Ok(count)
}

/// Packets with a valid FCS that coppa's TNC receive chain decodes from `samples` at `rate`.
///
/// When `rate` is not 48 kHz the samples go through `coppa_audio::resampler::ResamplingSource`
/// first, the type `LiveSource` uses for a card that cannot run at 48 kHz
/// (`crates/coppa-audio/src/cpal_backend.rs`). The demodulator then takes `BLOCK`-sample reads, as
/// `run_demod_worker` in `crates/coppa-daemon/src/tnc.rs` does. Every FCS-valid frame counts,
/// repeats included.
pub fn count_coppa_packets(samples: &[f32], rate: u32) -> Result<usize> {
    let src = SliceSource::new(samples, rate);
    if rate == ENGINE_RATE {
        drain_and_count(src)
    } else {
        drain_and_count(ResamplingSource::new(src, ENGINE_RATE)?)
    }
}

/// `count_coppa_packets` on `samples[offset..]` for each offset. An offset past the end counts an
/// empty input.
pub fn coppa_counts_by_offset(samples: &[f32], rate: u32, offsets: &[usize]) -> Result<Vec<usize>> {
    offsets
        .iter()
        .map(|&offset| count_coppa_packets(samples.get(offset..).unwrap_or(&[]), rate))
        .collect()
}

/// Header facts the report prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavInfo {
    pub file_name: String,
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    /// Sample frames (samples per channel).
    pub frames: u32,
}

/// Channel 0 of a WAV as f32, read through `coppa_audio::WavSource::open` (the file backend's
/// loader, which keeps channel 0), plus the header facts the report prints.
pub fn load_channel0(path: &Path) -> Result<(Vec<f32>, WavInfo)> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("{}: cannot open as WAV", path.display()))?;
    let spec = reader.spec();
    let frames = reader.duration();
    drop(reader);
    if frames == 0 {
        bail!("{}: the WAV holds no samples", path.display());
    }
    let mut src = WavSource::open(path)?;
    let mut samples = vec![0.0f32; src.total_samples()];
    let n = src.read(&mut samples)?;
    samples.truncate(n);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Ok((
        samples,
        WavInfo {
            file_name,
            rate: spec.sample_rate,
            channels: spec.channels,
            bits: spec.bits_per_sample,
            frames,
        },
    ))
}

// ---------------------------------------------------------------------------------------------
// Direwolf's output
// ---------------------------------------------------------------------------------------------

/// Removes ANSI CSI sequences (ESC `[`, parameter bytes, then a final byte in 0x40..=0x7E).
/// `atest` colours its output even into a pipe.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// True when `line` (already stripped) starts `[<channel>] ` or `[<channel>.<digits>] `.
fn is_frame_line(line: &str, channel: u8) -> bool {
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(channel.to_string().as_str()) else {
        return false;
    };
    let rest = match rest.strip_prefix('.') {
        Some(after_dot) => {
            let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            &after_dot[digits..]
        }
        None => rest,
    };
    rest.starts_with("] ")
}

/// Lines that are decoded frames on `channel`: `[ch] ` or `[ch.N] ` at the line start, after
/// `strip_ansi`. Status text that merely begins with `[` (for example `[224]*...`) does not count.
pub fn count_frame_lines(output: &str, channel: u8) -> usize {
    output
        .lines()
        .filter(|line| is_frame_line(&strip_ansi(line), channel))
        .count()
}

/// The N of `atest`'s `N packets decoded in ...` summary line.
pub fn atest_summary_count(output: &str) -> Option<usize> {
    output.lines().find_map(|line| {
        let line = strip_ansi(line);
        let idx = line.find(" packets decoded in")?;
        line[..idx].trim().parse().ok()
    })
}

/// The third comma-separated field of the banner line
/// `Channel <ch>: 1200 baud, AFSK 1200 & 2200 Hz, A+, 44100 sample rate.`
pub fn channel_profile(output: &str, channel: u8) -> Option<String> {
    let prefix = format!("Channel {channel}: ");
    output.lines().find_map(|line| {
        let line = strip_ansi(line);
        if !line.starts_with(&prefix) {
            return None;
        }
        let field = line.split(',').nth(2)?.trim();
        (!field.is_empty()).then(|| field.to_owned())
    })
}

/// The first line starting `Dire Wolf ` that contains `version`.
pub fn direwolf_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = strip_ansi(line);
        let line = line.trim();
        (line.starts_with("Dire Wolf ") && line.contains("version")).then(|| line.to_owned())
    })
}

/// Smallest whole packet count at or above 95 percent of `reference`.
pub fn bar_95(reference: usize) -> usize {
    (95 * reference).div_ceil(100)
}

/// `100 * count / reference`, or `None` when the reference decoded nothing.
pub fn percent_of(count: usize, reference: usize) -> Option<f64> {
    (reference != 0).then(|| 100.0 * count as f64 / reference as f64)
}

/// One Direwolf program's result on the input.
#[derive(Debug, Clone, PartialEq)]
pub struct DecoderRun {
    /// Channel 0 frame lines in its output.
    pub packets: usize,
    /// Demodulator profile from its `Channel 0:` banner line.
    pub profile: Option<String>,
    /// Its `Dire Wolf version ...` banner line (`atest` prints none).
    pub version: Option<String>,
    /// Its stdout followed by its stderr, raw.
    pub output: Vec<u8>,
}

/// The benchmark's report; `Display` renders the seven report lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub input: WavInfo,
    pub offsets: Vec<usize>,
    /// coppa's count at each of `offsets`; the first entry (offset 0) is the headline count.
    pub coppa_by_offset: Vec<usize>,
    pub direwolf: DecoderRun,
    pub atest: DecoderRun,
}

impl Report {
    /// coppa's headline count: the count at the first offset (0).
    pub fn coppa_packets(&self) -> usize {
        self.coppa_by_offset.first().copied().unwrap_or(0)
    }
}

/// One `coppa / <name>` line: the percentage and the 95 percent bar, or `n/a` when `name`
/// decoded nothing. `pad` aligns the two lines.
fn ratio_line(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    pad: &str,
    coppa: usize,
    run: &DecoderRun,
) -> fmt::Result {
    let label = format!("coppa / {name}:{pad}");
    match percent_of(coppa, run.packets) {
        Some(p) => {
            let bar = bar_95(run.packets);
            let verdict = if coppa >= bar { "met" } else { "not met" };
            writeln!(f, "{label}{p:.1}% (95% bar: {bar} packets, {verdict})")
        }
        None => writeln!(f, "{label}n/a ({name} decoded 0 packets)"),
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let input = &self.input;
        let seconds = if input.rate == 0 {
            0.0
        } else {
            f64::from(input.frames) / f64::from(input.rate)
        };
        writeln!(
            f,
            "input: {}, {} Hz, {} channels, {}-bit, {seconds:.1} s",
            input.file_name, input.rate, input.channels, input.bits
        )?;
        let coppa = self.coppa_packets();
        if input.rate == ENGINE_RATE {
            writeln!(
                f,
                "coppa:    {coppa} packets (AFSK 1200 demodulator, input at {ENGINE_RATE} Hz)"
            )?;
        } else {
            writeln!(
                f,
                "coppa:    {coppa} packets (AFSK 1200 demodulator, resampled {} -> {ENGINE_RATE} Hz)",
                input.rate
            )?;
        }
        let pairs: Vec<String> = self
            .offsets
            .iter()
            .zip(&self.coppa_by_offset)
            .map(|(o, n)| format!("{o}:{n}"))
            .collect();
        match (
            self.coppa_by_offset.iter().min(),
            self.coppa_by_offset.iter().max(),
        ) {
            (Some(lo), Some(hi)) => writeln!(
                f,
                "coppa by start offset: {} (spread {lo} to {hi})",
                pairs.join(" ")
            )?,
            _ => writeln!(f, "coppa by start offset: none")?,
        }
        writeln!(
            f,
            "direwolf: {} packets ({}, profile {})",
            self.direwolf.packets,
            self.direwolf
                .version
                .as_deref()
                .unwrap_or("version unknown"),
            self.direwolf.profile.as_deref().unwrap_or("unknown")
        )?;
        writeln!(
            f,
            "atest:    {} packets (profile {})",
            self.atest.packets,
            self.atest.profile.as_deref().unwrap_or("unknown")
        )?;
        ratio_line(f, "direwolf", " ", coppa, &self.direwolf)?;
        ratio_line(f, "atest", "    ", coppa, &self.atest)
    }
}

// ---------------------------------------------------------------------------------------------
// Running Direwolf
// ---------------------------------------------------------------------------------------------

/// The `direwolf` config: input on stdin at the WAV's rate, no network ports, nothing else, so
/// every demodulator setting is Direwolf's own default.
pub fn direwolf_config(rate: u32) -> String {
    format!("ADEVICE stdin null\nACHANNELS 1\nARATE {rate}\nAGWPORT 0\nKISSPORT 0\n")
}

/// Maps a spawn failure to an error naming the flag that picks the program.
fn spawn_error(program: &Path, flag: &str, err: io::Error) -> anyhow::Error {
    if err.kind() == io::ErrorKind::NotFound {
        anyhow!(
            "{}: not found; install Direwolf 1.7 (Debian/Ubuntu: apt install direwolf) or pass {flag} PATH",
            program.display()
        )
    } else {
        anyhow!("{}: cannot start: {err}", program.display())
    }
}

/// The last non-empty line of a program's output, for error messages.
fn last_line(text: &str) -> String {
    text.lines()
        .map(|line| strip_ansi(line).trim().to_owned())
        .rfind(|line| !line.is_empty())
        .unwrap_or_else(|| "(no output)".to_owned())
}

/// Stdout followed by stderr, and an error when the program did not exit successfully.
fn collect_output(
    program: &Path,
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: &[u8],
) -> Result<Vec<u8>> {
    let mut output = stdout;
    output.extend_from_slice(stderr);
    if !status.success() {
        bail!(
            "{} exited with {status}: {}",
            program.display(),
            last_line(&String::from_utf8_lossy(&output))
        );
    }
    Ok(output)
}

/// Writes every `channels`-th sample, starting at channel 0, to `sink` as 16-bit little-endian.
fn write_channel0<R: io::Read, W: Write>(
    reader: hound::WavReader<R>,
    channels: usize,
    mut sink: W,
) -> io::Result<()> {
    let channels = channels.max(1);
    let mut chunk = Vec::with_capacity(STDIN_CHUNK);
    for (i, sample) in reader.into_samples::<i16>().enumerate() {
        let sample = sample.map_err(io::Error::other)?;
        if i % channels == 0 {
            chunk.extend_from_slice(&sample.to_le_bytes());
            if chunk.len() >= STDIN_CHUNK {
                sink.write_all(&chunk)?;
                chunk.clear();
            }
        }
    }
    if !chunk.is_empty() {
        sink.write_all(&chunk)?;
    }
    sink.flush()
}

/// Opens `wav` and checks that it is 16-bit PCM, the only input `run_direwolf` streams to
/// `direwolf`. The `afsk_vs_direwolf` diagnostic calls it before coppa's pass, so a wrong file
/// fails at once instead of after minutes of decoding.
pub fn open_pcm16(wav: &Path) -> Result<hound::WavReader<io::BufReader<std::fs::File>>> {
    let reader = hound::WavReader::open(wav)
        .with_context(|| format!("{}: cannot open as WAV", wav.display()))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        bail!("{}: direwolf input must be 16-bit PCM", wav.display());
    }
    Ok(reader)
}

/// Runs `program -c <work_dir>/direwolf.conf -t 0` with channel 0 of `wav` on stdin as 16-bit
/// little-endian samples, and counts its channel 0 frame lines.
pub fn run_direwolf(program: &Path, wav: &Path, work_dir: &Path) -> Result<DecoderRun> {
    let reader = open_pcm16(wav)?;
    let spec = reader.spec();
    let conf = work_dir.join("direwolf.conf");
    std::fs::write(&conf, direwolf_config(spec.sample_rate))
        .with_context(|| format!("writing {}", conf.display()))?;

    let mut child = Command::new(program)
        .arg("-c")
        .arg(&conf)
        .arg("-t")
        .arg("0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(program, "--direwolf", e))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("{}: no stdin pipe", program.display()))?;
    // Write stdin from its own thread while this one drains stdout and stderr, so neither side
    // blocks on a full pipe.
    let channels = usize::from(spec.channels);
    let writer = std::thread::spawn(move || write_channel0(reader, channels, stdin));
    let out = child
        .wait_with_output()
        .with_context(|| format!("waiting for {}", program.display()))?;
    let written = writer
        .join()
        .map_err(|_| anyhow!("the thread writing {}'s stdin panicked", program.display()))?;
    match written {
        // Rust ignores SIGPIPE, so a program that exits before reading all its input shows up
        // here as BrokenPipe; its exit status below says whether that was a failure.
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => {
            return Err(anyhow!(e).context(format!("writing samples to {}", program.display())));
        }
        _ => {}
    }

    let output = collect_output(program, out.status, out.stdout, &out.stderr)?;
    let text = String::from_utf8_lossy(&output);
    Ok(DecoderRun {
        packets: count_frame_lines(&text, 0),
        profile: channel_profile(&text, 0),
        version: direwolf_version(&text),
        output,
    })
}

/// Runs `program <wav>` (Direwolf's `atest`, no options) and cross-checks its channel 0 frame
/// lines against its `N packets decoded in` summary.
pub fn run_atest(program: &Path, wav: &Path) -> Result<DecoderRun> {
    let out = Command::new(program)
        .arg(wav)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| spawn_error(program, "--atest", e))?;
    let output = collect_output(program, out.status, out.stdout, &out.stderr)?;
    let text = String::from_utf8_lossy(&output);
    let packets = count_frame_lines(&text, 0);
    let summary = atest_summary_count(&text).ok_or_else(|| {
        anyhow!(
            "{}: no `N packets decoded in` line in its output",
            program.display()
        )
    })?;
    if summary != packets {
        bail!(
            "{}: its summary says {summary} packets decoded in total, but {packets} channel 0 frame lines were found",
            program.display()
        );
    }
    Ok(DecoderRun {
        packets,
        profile: channel_profile(&text, 0),
        version: None,
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_codec::afsk::{append_fcs, modulate};
    use std::path::PathBuf;

    /// `n` FCS-framed packets at 48 kHz, half scale, 0.1 s of silence after each.
    fn synthetic_48k(n: usize) -> Vec<f32> {
        let mut v = Vec::new();
        for i in 0..n {
            let info = format!("N0CALL>APRS:>cop112 synthetic {i}");
            v.extend(
                modulate(&append_fcs(info.as_bytes()))
                    .iter()
                    .map(|x| x * 0.5),
            );
            v.extend(std::iter::repeat_n(0.0f32, 4_800));
        }
        v
    }

    /// The same resampler type the TNC uses, run in the other direction.
    fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
        let mut src = ResamplingSource::new(SliceSource::new(samples, from), to).unwrap();
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; BLOCK];
        loop {
            let n = src.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    /// A per-test temp file removed on drop.
    struct TempFile(PathBuf);

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn counts_every_packet_at_48k() {
        assert_eq!(count_coppa_packets(&synthetic_48k(20), 48_000).unwrap(), 20);
    }

    #[test]
    fn resamples_44k1_input_like_the_tnc() {
        let s = resample(&synthetic_48k(20), 48_000, 44_100);
        assert_eq!(count_coppa_packets(&s, 44_100).unwrap(), 20);
    }

    #[test]
    fn unresampled_44k1_input_decodes_nothing() {
        let s = resample(&synthetic_48k(20), 48_000, 44_100);
        assert_eq!(count_coppa_packets(&s, 48_000).unwrap(), 0);
    }

    #[test]
    fn offset_counts_follow_the_offsets() {
        assert_eq!(START_OFFSETS[0], 0);
        assert_eq!(START_OFFSETS.len(), 8);
        assert!(START_OFFSETS.iter().all(|&o| o < 37));
        let s = synthetic_48k(3);
        let counts = coppa_counts_by_offset(&s, 48_000, &START_OFFSETS).unwrap();
        assert_eq!(counts.len(), 8);
        assert_eq!(counts[0], count_coppa_packets(&s, 48_000).unwrap());
    }

    #[test]
    fn offset_past_the_end_counts_zero() {
        assert_eq!(
            coppa_counts_by_offset(&[0.0; 10], 48_000, &[20]).unwrap(),
            vec![0]
        );
    }

    #[test]
    fn load_channel0_reads_the_left_channel() {
        let left = resample(&synthetic_48k(5), 48_000, 44_100);
        let name = format!("cop112_load_channel0_{}.wav", std::process::id());
        let path = TempFile(std::env::temp_dir().join(&name));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path.0, spec).unwrap();
        for &x in &left {
            w.write_sample((x.clamp(-1.0, 1.0) * 32_767.0) as i16)
                .unwrap();
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();

        let (samples, info) = load_channel0(&path.0).unwrap();
        assert_eq!(
            info,
            WavInfo {
                file_name: name,
                rate: 44_100,
                channels: 2,
                bits: 16,
                frames: left.len() as u32,
            }
        );
        assert_eq!(samples.len(), left.len());
        assert_eq!(count_coppa_packets(&samples, 44_100).unwrap(), 5);
    }

    pub(super) const ATEST_FIXTURE: &str = concat!(
        "\u{1b}[48;2;255;255;255m\u{1b}[0J\u{1b}[38;2;0;0;0m44100 samples per second.  16 bits per sample.  2 audio channels.\n",
        "273405888 audio bytes in file.  Duration = 1549.9 seconds.\n",
        "Fix Bits level = 0\n",
        "\u{1b}[38;2;0;192;0mChannel 0: 1200 baud, AFSK 1200 & 2200 Hz, A, 44100 sample rate.\n",
        "\u{1b}[38;2;0;192;0mChannel 1: 1200 baud, AFSK 1200 & 2200 Hz, A, 44100 sample rate.\n",
        "DECODED[1] 0:08.259 Digipeater N6EX-5 audio level = 26(3/1)     \n",
        "\u{1b}[38;2;0;192;0m[0] WA6YLB-4>APRS,N6EX-5*:$ULTW00000000----0000<0x0d><0x0a>\n",
        "DECODED[2] 0:10.142 KD6FVP-2 audio level = 31(4/1)     \n",
        "\u{1b}[38;2;0;192;0m[0] KD6FVP-2>APS224,N6EX-1,WIDE1:>152343z[224]*We know most of your faults!!!<0x0d>\n",
        "2 from track2.wav\n",
        "2 packets decoded in 13.946 seconds.  111.1 x realtime\n",
    );

    pub(super) const DIREWOLF_FIXTURE: &str = concat!(
        "Dire Wolf version 1.7\n",
        "Includes optional support for:  gpsd hamlib cm108-ptt\n",
        "Channel 0: 1200 baud, AFSK 1200 & 2200 Hz, A+, 44100 sample rate.\n",
        "Digipeater N6EX-5 audio level = 26(3/1)    __|||||||\n",
        "[0.5] WA6YLB-4>APRS,N6EX-5*:$ULTW00000000----0000<0x0d><0x0a>\n",
        "KD6FVP-2 audio level = 31(4/1)    __|||||__\n",
        "[0.4] KD6FVP-2>APS224,N6EX-1,WIDE1:>152343z[224]*We know most of your faults!!!<0x0d>\n",
        "Status Report\n",
        "[224]*We know most of your faults!!!\n",
        "[1.2] N0CALL>APRS:channel one is not counted\n",
        "End of file on stdin.  Exiting.\n",
    );

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\u{1b}[38;2;0;192;0m[0] A>B:x"), "[0] A>B:x");
        assert_eq!(strip_ansi("[0.4] plain line"), "[0.4] plain line");
    }

    #[test]
    fn frame_lines_count_channel_0_only() {
        assert_eq!(count_frame_lines(DIREWOLF_FIXTURE, 0), 2);
        assert_eq!(count_frame_lines(ATEST_FIXTURE, 0), 2);
    }

    #[test]
    fn frame_lines_survive_non_utf8_payloads() {
        let text = String::from_utf8_lossy(
            b"[0.4] WD6V-5>WB6JAR-10:=3330.60N/11711.80W-DIGI<0x0d>\x9f\x80\n",
        );
        assert_eq!(count_frame_lines(&text, 0), 1);
    }

    #[test]
    fn rejects_lookalike_lines() {
        for line in ["[0]x", "[0.] A", "[0.a] A", "[00] A", " [0] A"] {
            assert_eq!(count_frame_lines(line, 0), 0, "{line:?}");
        }
    }

    #[test]
    fn atest_summary_is_parsed() {
        assert_eq!(atest_summary_count(ATEST_FIXTURE), Some(2));
        assert_eq!(atest_summary_count(DIREWOLF_FIXTURE), None);
    }

    #[test]
    fn profiles_come_from_the_banner() {
        assert_eq!(channel_profile(DIREWOLF_FIXTURE, 0).as_deref(), Some("A+"));
        assert_eq!(channel_profile(ATEST_FIXTURE, 0).as_deref(), Some("A"));
        assert_eq!(channel_profile("no banner here\n", 0), None);
    }

    #[test]
    fn version_comes_from_the_banner() {
        assert_eq!(
            direwolf_version(DIREWOLF_FIXTURE).as_deref(),
            Some("Dire Wolf version 1.7")
        );
        assert_eq!(direwolf_version(ATEST_FIXTURE), None);
    }

    #[test]
    fn bar_and_percent() {
        assert_eq!(bar_95(1000), 950);
        assert_eq!(bar_95(982), 933);
        assert_eq!(bar_95(0), 0);
        assert!((percent_of(412, 1000).unwrap() - 41.2).abs() < 1e-9);
        assert_eq!(percent_of(1, 0), None);
    }

    fn baseline_report(rate: u32) -> Report {
        Report {
            input: WavInfo {
                file_name: "track2.wav".to_owned(),
                rate,
                channels: 2,
                bits: 16,
                frames: 68_351_472,
            },
            offsets: START_OFFSETS.to_vec(),
            coppa_by_offset: vec![412, 416, 398, 399, 390, 397, 388, 407],
            direwolf: DecoderRun {
                packets: 1000,
                profile: Some("A+".to_owned()),
                version: Some("Dire Wolf version 1.7".to_owned()),
                output: Vec::new(),
            },
            atest: DecoderRun {
                packets: 982,
                profile: Some("A".to_owned()),
                version: None,
                output: Vec::new(),
            },
        }
    }

    #[test]
    fn report_renders_the_measured_baseline() {
        let expected = concat!(
            "input: track2.wav, 44100 Hz, 2 channels, 16-bit, 1549.9 s\n",
            "coppa:    412 packets (AFSK 1200 demodulator, resampled 44100 -> 48000 Hz)\n",
            "coppa by start offset: 0:412 5:416 10:398 15:399 20:390 25:397 30:388 35:407 (spread 388 to 416)\n",
            "direwolf: 1000 packets (Dire Wolf version 1.7, profile A+)\n",
            "atest:    982 packets (profile A)\n",
            "coppa / direwolf: 41.2% (95% bar: 950 packets, not met)\n",
            "coppa / atest:    42.0% (95% bar: 933 packets, not met)\n",
        );
        assert_eq!(baseline_report(44_100).to_string(), expected);
    }

    #[test]
    fn report_at_48k_says_no_resampling() {
        let text = baseline_report(48_000).to_string();
        assert!(
            text.contains("coppa:    412 packets (AFSK 1200 demodulator, input at 48000 Hz)\n"),
            "{text}"
        );
    }

    #[test]
    fn report_handles_zero_and_met() {
        let mut r = baseline_report(44_100);
        r.direwolf.packets = 0;
        assert!(r
            .to_string()
            .contains("coppa / direwolf: n/a (direwolf decoded 0 packets)\n"));

        let mut r = baseline_report(44_100);
        r.coppa_by_offset[0] = 960;
        assert!(r
            .to_string()
            .contains("coppa / direwolf: 96.0% (95% bar: 950 packets, met)\n"));

        let mut r = baseline_report(44_100);
        r.direwolf.profile = None;
        r.direwolf.version = None;
        r.atest.profile = None;
        let text = r.to_string();
        assert!(
            text.contains("direwolf: 1000 packets (version unknown, profile unknown)\n"),
            "{text}"
        );
        assert!(
            text.contains("atest:    982 packets (profile unknown)\n"),
            "{text}"
        );
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::tests::{ATEST_FIXTURE, DIREWOLF_FIXTURE};
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Held while a test writes and runs a fake program: writing an executable while another
    /// test thread forks can make `exec` fail with "Text file busy".
    static SPAWN_LOCK: Mutex<()> = Mutex::new(());

    /// A per-test scratch directory removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("cop112_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn wav(&self, name: &str, spec: hound::WavSpec, frames: usize) -> PathBuf {
            let path = self.0.join(name);
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..frames * usize::from(spec.channels) {
                match spec.sample_format {
                    hound::SampleFormat::Int => w.write_sample((i % 100) as i16).unwrap(),
                    hound::SampleFormat::Float => w.write_sample(0.0f32).unwrap(),
                }
            }
            w.finalize().unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pcm16(channels: u16) -> hound::WavSpec {
        hound::WavSpec {
            channels,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        }
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn direwolf_gets_channel0_and_an_io_only_config() {
        let _guard = lock();
        let s = Scratch::new("dw_config");
        let wav = s.wav("in.wav", pcm16(2), 1000);
        let fake = s.script(
            "direwolf",
            &format!(
                "[ \"$1\" = -c ] || exit 3\n\
                 [ \"$3\" = -t ] || exit 3\n\
                 [ \"$4\" = 0 ] || exit 3\n\
                 diff \"$2\" - >/dev/null <<'CONF' || exit 3\n\
                 ADEVICE stdin null\nACHANNELS 1\nARATE 44100\nAGWPORT 0\nKISSPORT 0\n\
                 CONF\n\
                 cat <<'OUT'\n{DIREWOLF_FIXTURE}OUT\n\
                 echo \"stdin_bytes=$(wc -c | tr -d ' ')\" >&2\n"
            ),
        );
        let run = run_direwolf(&fake, &wav, &s.0).unwrap();
        assert_eq!(run.packets, 2);
        assert_eq!(run.profile.as_deref(), Some("A+"));
        assert_eq!(run.version.as_deref(), Some("Dire Wolf version 1.7"));
        let text = String::from_utf8_lossy(&run.output);
        assert!(text.contains("stdin_bytes=2000"), "{text}");
    }

    #[test]
    fn direwolf_does_not_deadlock_on_large_output() {
        let _guard = lock();
        let s = Scratch::new("dw_deadlock");
        let wav = s.wav("in.wav", pcm16(1), 200_000);
        let fake = s.script(
            "direwolf",
            "dd if=/dev/zero bs=1000 count=100 2>/dev/null | tr '\\000' x\n\
             cat >/dev/null\n",
        );
        let work = s.0.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_direwolf(&fake, &wav, &work).map(|r| r.output.len()));
        });
        let len = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("run_direwolf deadlocked")
            .unwrap();
        assert_eq!(len, 100_000);
    }

    #[test]
    fn direwolf_failure_is_an_error() {
        let _guard = lock();
        let s = Scratch::new("dw_failure");
        let wav = s.wav("in.wav", pcm16(1), 1000);
        let fake = s.script("direwolf", "echo 'bad config'\nexit 1\n");
        let err = format!("{:#}", run_direwolf(&fake, &wav, &s.0).unwrap_err());
        assert!(err.contains("exited with"), "{err}");
        assert!(err.contains("bad config"), "{err}");
    }

    #[test]
    fn missing_program_names_the_flag() {
        let s = Scratch::new("missing");
        let wav = s.wav("in.wav", pcm16(1), 100);
        let err = format!(
            "{:#}",
            run_direwolf(Path::new("/nonexistent/direwolf"), &wav, &s.0).unwrap_err()
        );
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("--direwolf"), "{err}");
        let err = format!(
            "{:#}",
            run_atest(Path::new("/nonexistent/atest"), &wav).unwrap_err()
        );
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("--atest"), "{err}");
    }

    #[test]
    fn direwolf_rejects_non_16_bit_input() {
        let s = Scratch::new("dw_float");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let wav = s.wav("in.wav", spec, 100);
        let err = format!(
            "{:#}",
            run_direwolf(Path::new("/nonexistent/direwolf"), &wav, &s.0).unwrap_err()
        );
        assert!(err.contains("16-bit PCM"), "{err}");
    }

    #[test]
    fn atest_counts_and_cross_checks() {
        let _guard = lock();
        let s = Scratch::new("atest");
        let wav = s.wav("in.wav", pcm16(2), 100);
        let wav_arg = wav.display().to_string();

        let fake = s.script(
            "atest",
            &format!(
                "[ \"$#\" = 1 ] || exit 3\n\
                 [ \"$1\" = '{wav_arg}' ] || exit 3\n\
                 cat <<'OUT'\n{ATEST_FIXTURE}OUT\n"
            ),
        );
        let run = run_atest(&fake, &wav).unwrap();
        assert_eq!(run.packets, 2);
        assert_eq!(run.profile.as_deref(), Some("A"));
        assert_eq!(run.version, None);

        let wrong = ATEST_FIXTURE.replace("2 packets decoded in", "3 packets decoded in");
        let fake = s.script("atest_wrong", &format!("cat <<'OUT'\n{wrong}OUT\n"));
        let err = format!("{:#}", run_atest(&fake, &wav).unwrap_err());
        assert!(err.contains("says 3 packets"), "{err}");
        assert!(err.contains("but 2 channel 0 frame lines"), "{err}");

        let nosummary = ATEST_FIXTURE.replace("2 packets decoded in", "2 packets seen in");
        let fake = s.script("atest_nosummary", &format!("cat <<'OUT'\n{nosummary}OUT\n"));
        let err = format!("{:#}", run_atest(&fake, &wav).unwrap_err());
        assert!(err.contains("packets decoded in"), "{err}");
    }
}
