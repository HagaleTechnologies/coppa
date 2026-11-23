//! Audio format and block accounting for the userspace cable.

use anyhow::{bail, Result};

pub const BLOCK: usize = 960;
pub const MUTE_HOLD_TICKS: u8 = 5;

pub fn validate_playback(samples: &[f32]) -> Result<()> {
    if samples.iter().any(|&s| !s.is_finite() || s.abs() > 1.0) {
        bail!("station playback is not f32le 48000 Hz mono audio");
    }
    Ok(())
}

pub fn take_whole_block(accumulator: &mut Vec<f32>) -> Option<Vec<f32>> {
    if accumulator.len() < BLOCK {
        return None;
    }
    Some(accumulator.drain(..BLOCK).collect())
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
        pub fn start(
            dir: &Path,
            config: Option<WattersonConfig>,
            snr_db: Option<i32>,
            ref_power: [f32; 2],
            seed: u64,
        ) -> Result<Self> {
            let a_in = open(&dir.join("ab_in"))?;
            let b_in = open(&dir.join("ba_in"))?;
            let a_out = open(&dir.join("ab_out"))?;
            let b_out = open(&dir.join("ba_out"))?;
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
                let _ = handle.join();
            }
        }
    }

    fn open(path: &Path) -> Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(path)
            .with_context(|| format!("opening FIFO {}", path.display()))
    }

    fn read_samples(file: &mut File, carry: &mut Vec<u8>, samples: &mut Vec<f32>) -> Result<()> {
        let mut bytes = [0u8; 8192];
        // The ALSA file PCM has a null slave that can run faster than wall
        // time. Read at most one 20 ms block per tick so the pipe supplies
        // backpressure and the modem's callback stays on the channel clock.
        while samples.len() + carry.len() / 4 < BLOCK {
            let needed = (BLOCK - samples.len()) * 4 - carry.len();
            let to_read = needed.min(bytes.len());
            match file.read(&mut bytes[..to_read]) {
                Ok(0) => break,
                Ok(n) => carry.extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        let whole = carry.len() / 4 * 4;
        for part in carry[..whole].chunks_exact(4) {
            samples.push(f32::from_le_bytes(part.try_into()?));
        }
        carry.drain(..whole);
        Ok(())
    }

    fn write_samples(file: &mut File, samples: &[f32]) -> Result<bool> {
        let bytes: Vec<u8> = samples.iter().flat_map(|x| x.to_le_bytes()).collect();
        match file.write(&bytes) {
            Ok(n) => Ok(n == bytes.len()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn run_loop(
        mut inputs: [File; 2],
        mut outputs: [File; 2],
        channels: &mut [PersistentChannel; 2],
        stop: &AtomicBool,
        counts: &Counters,
        marker_dir: &Path,
    ) -> Result<()> {
        let mut bytes = [Vec::new(), Vec::new()];
        let mut samples = [Vec::new(), Vec::new()];
        let mut hold = [0u8; 2];
        let mut seen = [false; 2];
        let period = Duration::from_millis(20);
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let counting = counts.counting.load(Ordering::Relaxed)
                || (marker_dir.join("probe-start").exists()
                    && !marker_dir.join("probe-end").exists());
            for i in 0..2 {
                read_samples(&mut inputs[i], &mut bytes[i], &mut samples[i])?;
            }
            let mut blocks = [vec![0.0; BLOCK], vec![0.0; BLOCK]];
            for i in 0..2 {
                if let Some(block) = take_whole_block(&mut samples[i]) {
                    blocks[i] = block;
                    seen[i] = true;
                } else if counting && seen[i] {
                    counts.starved.fetch_add(1, Ordering::Relaxed);
                }
                validate_playback(&blocks[i])?;
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
            if !write_samples(&mut outputs[0], &ab)? && counting {
                counts.overflow.fetch_add(1, Ordering::Relaxed);
            }
            if !write_samples(&mut outputs[1], &ba)? && counting && seen[1] {
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
        assert!(take_whole_block(&mut a).is_none());
        a.extend(vec![0.25; 460]);
        assert_eq!(take_whole_block(&mut a).unwrap().len(), BLOCK);
    }
    #[test]
    fn non_f32_playback_is_refused() {
        assert!(validate_playback(&[f32::INFINITY]).is_err());
        assert!(validate_playback(&[3.4e12]).is_err());
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
        let relay = super::io::Relay::start(&cable.dir, None, None, [1.0, 1.0], 7).unwrap();
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
        let relay = super::io::Relay::start(&cable.dir, None, None, [1.0, 1.0], 7).unwrap();
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
