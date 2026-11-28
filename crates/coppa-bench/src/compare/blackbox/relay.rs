//! Audio format and block accounting for the userspace cable.

use super::native::NativeFormat;
use anyhow::{bail, Result};

pub const BLOCK: usize = 960;
pub const MUTE_HOLD_TICKS: u8 = 5;

/// Refuses f32 playback that cannot be audio. Every s16 code is a valid
/// sample, so only an f32 declaration can be caught this way.
pub fn validate_playback(samples: &[f32], native: NativeFormat) -> Result<()> {
    if samples.iter().any(|&s| !s.is_finite() || s.abs() > 1.0) {
        bail!(
            "station playback is not {} {} Hz mono audio",
            native.format.name(),
            native.rate
        );
    }
    Ok(())
}

/// Takes one whole tick of `block` native samples, never a padded partial one.
pub fn take_whole_block(accumulator: &mut Vec<f32>, block: usize) -> Option<Vec<f32>> {
    if accumulator.len() < block {
        return None;
    }
    Some(accumulator.drain(..block).collect())
}

pub fn mute_for_own_playback(playback: &[f32], hold: &mut u8) -> bool {
    let active = playback
        .iter()
        .any(|&s| s.abs() > 10.0f32.powf(-50.0 / 20.0));
    if active {
        *hold = MUTE_HOLD_TICKS;
        true
    } else if *hold > 0 {
        *hold -= 1;
        true
    } else {
        false
    }
}

#[cfg(target_os = "linux")]
pub mod io {
    use super::super::native::{Capture, Converter, NativeFormat, SampleFormat};
    use super::{mute_for_own_playback, take_whole_block, validate_playback, BLOCK};
    use anyhow::{Context, Result};
    use coppa_channel::persistent::{PersistentChannel, SnrProfile};
    use coppa_channel::watterson::WattersonConfig;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    const O_NONBLOCK: i32 = 0o4000;

    #[derive(Default)]
    pub struct Counters {
        pub starved: AtomicU64,
        pub overflow: AtomicU64,
        pub counting: AtomicBool,
        power: Mutex<[(f64, u64); 2]>,
    }

    impl Counters {
        pub fn reset_power(&self) {
            *self.power.lock().unwrap() = [(0.0, 0); 2];
        }
        pub fn ref_power(&self) -> [f32; 2] {
            let p = self.power.lock().unwrap();
            [0, 1].map(|i| {
                if p[i].1 > 0 {
                    (p[i].0 / p[i].1 as f64) as f32
                } else {
                    0.0
                }
            })
        }
    }

    pub struct Relay {
        stop: Arc<AtomicBool>,
        pub counts: Arc<Counters>,
        thread: Option<JoinHandle<Result<()>>>,
    }

    impl Relay {
        /// Starts the cable's clock at unity capture gain, the path the
        /// integrity probe and the calibration use. Each station plays and
        /// captures in `native`; the channel runs at f32 48000 Hz between
        /// conversions.
        pub fn start(
            dir: &Path,
            native: NativeFormat,
            config: Option<WattersonConfig>,
            snr_db: Option<i32>,
            ref_power: [f32; 2],
            seed: u64,
        ) -> Result<Self> {
            Self::start_capture(dir, Capture::unity(native), config, snr_db, ref_power, seed)
        }

        /// The trial entry (COP-256): the same cable carrying a fixed
        /// capture gain, one value for both directions of the whole trial.
        /// The probe and the calibration stay on [`Relay::start`].
        pub(in crate::compare::blackbox) fn start_capture(
            dir: &Path,
            capture: Capture,
            config: Option<WattersonConfig>,
            snr_db: Option<i32>,
            ref_power: [f32; 2],
            seed: u64,
        ) -> Result<Self> {
            let native = capture.native;
            let a_in = open(&dir.join("ab_in"), native)?;
            let b_in = open(&dir.join("ba_in"), native)?;
            let a_out = open(&dir.join("ab_out"), native)?;
            let b_out = open(&dir.join("ba_out"), native)?;
            let stop = Arc::new(AtomicBool::new(false));
            let counts = Arc::new(Counters::default());
            let st = Arc::clone(&stop);
            let ct = Arc::clone(&counts);
            let marker_dir = dir.to_path_buf();
            let thread = thread::spawn(move || {
                let fading = config.as_ref();
                let snr = snr_db.map(|v| SnrProfile::constant(v as f32));
                let mut channels = [
                    PersistentChannel::new(48000.0, fading, snr.clone(), ref_power[0], seed, 0),
                    PersistentChannel::new(48000.0, fading, snr, ref_power[1], seed, 1),
                ];
                run_loop(
                    [a_in, b_in],
                    [a_out, b_out],
                    capture,
                    &mut channels,
                    &st,
                    &ct,
                    &marker_dir,
                )
            });
            Ok(Self {
                stop,
                counts,
                thread: Some(thread),
            })
        }
        /// True once the relay thread has exited, which before `stop` means
        /// it failed (for example on playback that is not f32 audio).
        pub fn is_dead(&self) -> bool {
            self.thread.as_ref().is_none_or(JoinHandle::is_finished)
        }
        pub fn stop(mut self) -> Result<()> {
            self.stop.store(true, Ordering::Relaxed);
            self.thread
                .take()
                .unwrap()
                .join()
                .map_err(|_| anyhow::anyhow!("relay panicked"))?
        }
    }

    impl Drop for Relay {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.thread.take() {
                // An error returned past a relay that already stopped (a
                // station that cannot open its FIFO after an s16le capture
                // was refused, say) would otherwise hide the relay's reason.
                if let Some(reason) = dropped_failure(handle.join()) {
                    eprintln!("{reason}");
                }
            }
        }
    }

    /// What a relay dropped without `stop` reports, if its thread failed.
    pub(super) fn dropped_failure(joined: thread::Result<Result<()>>) -> Option<String> {
        match joined {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(format!("compare blackbox: the cable relay stopped: {e:#}")),
            Err(_) => Some("compare blackbox: the cable relay panicked".into()),
        }
    }

    fn open(path: &Path, native: NativeFormat) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(path)
            .with_context(|| format!("opening FIFO {}", path.display()))?;
        if let Some(bytes) = native.pipe_bytes() {
            rustix::pipe::fcntl_setpipe_size(&file, bytes)
                .with_context(|| format!("sizing FIFO {} to {bytes} bytes", path.display()))?;
        }
        Ok(file)
    }

    fn read_samples(
        file: &mut File,
        native: NativeFormat,
        carry: &mut Vec<u8>,
        samples: &mut Vec<f32>,
    ) -> Result<()> {
        let mut bytes = [0u8; 8192];
        let width = native.format.bytes();
        let block = native.block();
        // The ALSA file PCM has a null slave that can run faster than wall
        // time. Read at most one 20 ms block per tick so the pipe supplies
        // backpressure and the modem's callback stays on the channel clock.
        while samples.len() + carry.len() / width < block {
            let needed = (block - samples.len()) * width - carry.len();
            let to_read = needed.min(bytes.len());
            match file.read(&mut bytes[..to_read]) {
                Ok(0) => break,
                Ok(n) => carry.extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        let whole = carry.len() / width * width;
        native.format.decode(&carry[..whole], samples);
        carry.drain(..whole);
        Ok(())
    }

    fn write_samples(file: &mut File, format: SampleFormat, samples: &[f32]) -> Result<bool> {
        let mut bytes = Vec::with_capacity(samples.len() * format.bytes());
        format.encode(samples, &mut bytes)?;
        match file.write(&bytes) {
            Ok(n) => Ok(n == bytes.len()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn run_loop(
        mut inputs: [File; 2],
        mut outputs: [File; 2],
        capture: Capture,
        channels: &mut [PersistentChannel; 2],
        stop: &AtomicBool,
        counts: &Counters,
        marker_dir: &Path,
    ) -> Result<()> {
        let native = capture.native;
        let mut bytes = [Vec::new(), Vec::new()];
        let mut samples = [Vec::new(), Vec::new()];
        let mut hold = [0u8; 2];
        let mut seen = [false; 2];
        let mut converters = [Converter::new(native), Converter::new(native)];
        let block = native.block();
        let period = Duration::from_millis(20);
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let counting = counts.counting.load(Ordering::Relaxed)
                || (marker_dir.join("probe-start").exists()
                    && !marker_dir.join("probe-end").exists());
            for i in 0..2 {
                read_samples(&mut inputs[i], native, &mut bytes[i], &mut samples[i])?;
            }
            let mut blocks = [vec![0.0; BLOCK], vec![0.0; BLOCK]];
            for i in 0..2 {
                let mut played = vec![0.0; block];
                if let Some(whole) = take_whole_block(&mut samples[i], block) {
                    played = whole;
                    seen[i] = true;
                } else if counting && seen[i] {
                    counts.starved.fetch_add(1, Ordering::Relaxed);
                }
                validate_playback(&played, native)?;
                blocks[i] = converters[i].up(&played);
                let mut p = counts.power.lock().unwrap();
                for &s in &blocks[i] {
                    if s.abs() > 0.003 {
                        p[i].0 += f64::from(s).powi(2);
                        p[i].1 += 1;
                    }
                }
            }
            // Each station hears the other unless its own transmitter is active.
            let mute_a = mute_for_own_playback(&blocks[0], &mut hold[0]);
            let mute_b = mute_for_own_playback(&blocks[1], &mut hold[1]);
            let mut ab = channels[0].process(&blocks[0]);
            let mut ba = channels[1].process(&blocks[1]);
            if mute_b {
                ab.fill(0.0);
            }
            if mute_a {
                ba.fill(0.0);
            }
            // Station B captures A's channel output, and A captures B's.
            // COP-256: one fixed gain scales both captures after the
            // channel and before quantization, so the SNR is unchanged;
            // a block that still clips is refused by the encoder below.
            let mut ab = converters[1].down(&ab);
            let mut ba = converters[0].down(&ba);
            capture.scale(&mut ab);
            capture.scale(&mut ba);
            if !write_samples(&mut outputs[0], native.format, &ab)? && counting {
                counts.overflow.fetch_add(1, Ordering::Relaxed);
            }
            if !write_samples(&mut outputs[1], native.format, &ba)? && counting && seen[1] {
                counts.overflow.fetch_add(1, Ordering::Relaxed);
            }
            next += period;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else {
                next = now;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn half_duplex_mute_follows_own_playback() {
        let mut hold = 0;
        assert!(mute_for_own_playback(&[0.1], &mut hold));
        for _ in 0..5 {
            assert!(mute_for_own_playback(&[0.0], &mut hold));
        }
        assert!(!mute_for_own_playback(&[0.0], &mut hold));
    }
    #[test]
    fn partial_reads_are_never_padded_mid_block() {
        let mut a = vec![0.5; 500];
        assert!(take_whole_block(&mut a, BLOCK).is_none());
        a.extend(vec![0.25; 460]);
        assert_eq!(take_whole_block(&mut a, BLOCK).unwrap().len(), BLOCK);
        // COP-221: a 12000 Hz tick is 240 native samples.
        let mut b = vec![0.5; 239];
        assert!(take_whole_block(&mut b, 240).is_none());
        b.push(0.25);
        assert_eq!(take_whole_block(&mut b, 240).unwrap().len(), 240);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn a_dropped_relay_reports_why_it_stopped() {
        let refused =
            anyhow::anyhow!("s16le capture sample 1.5 is outside the representable range");
        let reason = io::dropped_failure(Ok(Err(refused))).unwrap();
        assert!(reason.contains("s16le capture sample 1.5"), "{reason}");
        assert_eq!(io::dropped_failure(Ok(Ok(()))), None);
    }
    #[test]
    fn non_f32_playback_is_refused() {
        let error = validate_playback(&[f32::INFINITY], NativeFormat::CHANNEL).unwrap_err();
        assert!(error.to_string().contains("f32le 48000 Hz"), "{error}");
        assert!(validate_playback(&[3.4e12], NativeFormat::CHANNEL).is_err());
    }

    /// COP-256 AC2: one gain scales signal and noise equally in both
    /// directions for a whole s16le trial, before quantization. Each pair
    /// replays the same seeded channel and converter state, so the decoded
    /// integer capture must be the pre-gain floating-point reference times
    /// the one amplitude multiplier, to within half an s16 code step plus
    /// float rounding.
    #[test]
    fn s16le_trial_gain_preserves_signal_and_noise() {
        use super::super::native::{Capture, Converter};
        use coppa_channel::persistent::{PersistentChannel, SnrProfile};

        let native = NativeFormat::declared(Some("s16le"), Some(12_000), Some(1)).unwrap();
        let power = [0.023_498, 0.020_689];
        let capture = Capture::trial(native, false, 0, power);
        assert_eq!(capture.gain_db, -7);
        let amplitude = capture.amplitude();
        let seed = 0x170c_0000u64;
        // The AC1 planning waveform: 350 ticks of five tones at 0.1 each.
        let signal: Vec<f32> = (0..350 * native.block())
            .map(|n| {
                [350.0, 700.0, 1050.0, 1400.0, 1750.0]
                    .iter()
                    .map(|&f| 0.1 * (std::f32::consts::TAU * f * n as f32 / 12_000.0).cos())
                    .sum()
            })
            .collect();
        let silence = vec![0.0f32; signal.len()];
        for direction in 0u8..2 {
            for (label, played) in [("signal+noise", &signal), ("noise-only", &silence)] {
                let mut gained = Vec::new();
                let mut reference = Vec::new();
                let mut bytes = Vec::new();
                for run in 0..2 {
                    let mut converter = Converter::new(native);
                    let mut channel = PersistentChannel::new(
                        48_000.0,
                        None,
                        Some(SnrProfile::constant(0.0)),
                        power[direction as usize],
                        seed,
                        direction,
                    );
                    for tick in played.chunks(native.block()) {
                        let out = channel.process(&converter.up(tick));
                        let mut block = converter.down(&out);
                        if run == 0 {
                            // The capture path, as run_loop applies it.
                            capture.scale(&mut block);
                            bytes.clear();
                            native.format.encode(&block, &mut bytes).unwrap();
                            native.format.decode(&bytes, &mut gained);
                        } else {
                            // The pre-gain reference.
                            reference.extend_from_slice(&block);
                        }
                    }
                }
                assert_eq!(
                    gained.len(),
                    reference.len(),
                    "{label} direction {direction}"
                );
                for (n, (&coded, &clean)) in gained.iter().zip(&reference).enumerate() {
                    assert!(
                        (coded - clean * amplitude).abs() <= 0.5 / 32_768.0 + 2e-7,
                        "{label} direction {direction} sample {n}: {coded} vs {}",
                        clean * amplitude
                    );
                }
            }
        }
    }

    #[cfg(all(feature = "cpal-backend", target_os = "linux"))]
    #[test]
    fn fifo_relay_passes_samples_through_on_a_noiseless_channel() {
        use std::fs::OpenOptions;
        use std::io::{Read, Write};
        use std::os::unix::fs::OpenOptionsExt;
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("coppa-relay-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cable = super::super::cable::Cable::create(&dir).unwrap();
        let relay =
            super::io::Relay::start(&cable.dir, NativeFormat::CHANNEL, None, None, [1.0, 1.0], 7)
                .unwrap();
        let mut playback = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("ab_in"))
            .unwrap();
        let mut capture = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0o4000)
            .open(dir.join("ab_out"))
            .unwrap();
        let sent: Vec<f32> = (0..BLOCK).map(|i| i as f32 / 100_000.0).collect();
        for sample in &sent {
            playback.write_all(&sample.to_le_bytes()).unwrap();
        }
        let mut bytes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut found = false;
        while Instant::now() < deadline && !found {
            let mut buf = [0u8; 8192];
            match capture.read(&mut buf) {
                Ok(n) => bytes.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("reading FIFO: {e}"),
            }
            let received: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|part| f32::from_le_bytes(part.try_into().unwrap()))
                .collect();
            found = received.windows(BLOCK).any(|window| window == sent);
            std::thread::sleep(Duration::from_millis(10));
        }
        relay.stop().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            found,
            "the noiseless FIFO cable did not return the 960-sample block"
        );
    }

    /// COP-221 AC1 through the real FIFOs: a station playing s16le at
    /// 12000 Hz is converted to the 48000 Hz channel and back, and its peer's
    /// capture passes the AC10 gate on a noiseless cable.
    #[cfg(all(feature = "cpal-backend", target_os = "linux"))]
    #[test]
    fn fifo_relay_carries_s16le_12k_within_the_integrity_gate() {
        use std::fs::OpenOptions;
        use std::io::{Read, Write};
        use std::os::unix::fs::OpenOptionsExt;
        use std::time::{Duration, Instant};

        let native = NativeFormat::declared(Some("s16le"), Some(12_000), Some(1)).unwrap();
        let dir =
            std::env::temp_dir().join(format!("coppa-relay-native-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cable = super::super::cable::Cable::create(&dir).unwrap();
        let relay = super::io::Relay::start(&cable.dir, native, None, None, [1.0, 1.0], 7).unwrap();
        let mut playback = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0o4000)
            .open(dir.join("ab_in"))
            .unwrap();
        let mut capture = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0o4000)
            .open(dir.join("ab_out"))
            .unwrap();
        // 1.5 s of the native probe; the gate needs at least one second.
        let sent: Vec<f32> = native.probe_signal()[..18_000].to_vec();
        let mut bytes = Vec::new();
        native.format.encode(&sent, &mut bytes).unwrap();
        let (mut written, mut received) = (0, Vec::new());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && received.len() < 2 * (sent.len() + 12_000) {
            if written < bytes.len() {
                match playback.write(&bytes[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("writing FIFO: {e}"),
                }
            }
            let mut buf = [0u8; 8192];
            match capture.read(&mut buf) {
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("reading FIFO: {e}"),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let starved = relay
            .counts
            .starved
            .load(std::sync::atomic::Ordering::Relaxed);
        relay.stop().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let mut samples = Vec::new();
        native.format.decode(&received, &mut samples);
        let metrics =
            super::super::integrity::analyse(&sent, &samples, 12_000, starved, 0).unwrap();
        assert!(metrics.correlation >= 0.999, "{metrics:?}");
        assert!(metrics.level_db.abs() <= 0.1, "{metrics:?}");
    }

    /// COP-224: a station that falls silent after its first block is starved
    /// on every tick, but the relay counts those ticks only while `counting`
    /// is on, so the window `run_trial` opens is the window the CSV reports.
    #[cfg(all(feature = "cpal-backend", target_os = "linux"))]
    #[test]
    fn starved_ticks_count_only_while_counting_is_on() {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let dir =
            std::env::temp_dir().join(format!("coppa-relay-count-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cable = super::super::cable::Cable::create(&dir).unwrap();
        let relay =
            super::io::Relay::start(&cable.dir, NativeFormat::CHANNEL, None, None, [1.0, 1.0], 7)
                .unwrap();
        let mut playback = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("ab_in"))
            .unwrap();
        // One whole block marks station A as seen; nothing follows it.
        let block: Vec<u8> = vec![0.0f32; BLOCK]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        playback.write_all(&block).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let before = relay.counts.starved.load(Ordering::Relaxed);
        relay.counts.counting.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut after = 0;
        while Instant::now() < deadline && after == 0 {
            std::thread::sleep(Duration::from_millis(20));
            after = relay.counts.starved.load(Ordering::Relaxed);
        }
        relay.stop().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(before, 0, "starved ticks counted while counting was off");
        assert!(after > 0, "no starved tick counted once counting was on");
    }
}
