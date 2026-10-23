//! Playback-progress bookkeeping for output streams.
//!
//! A TX caller cannot treat "the ring accepted my samples" or "the output
//! callback read them" as "they played": the callback hands samples to the
//! device ahead of the instant they reach the DAC (CPAL calls that instant the
//! predicted *playback* timestamp). The output callback therefore records,
//! for every callback that consumed real samples, the cumulative real-sample
//! count and the predicted instant the last of them finishes playing. Callers
//! read a coherent snapshot of both through [`PlaybackProgress`].
//!
//! Everything the callback touches here is allocation-free and lock-free: a
//! single-writer seqlock over two atomics, so a reader can never pair a new
//! count with an old deadline.

// The callback-side helpers are only wired into a real stream by the
// `cpal-backend` sink; without it they are exercised by this module's tests.
#![cfg_attr(not(feature = "cpal-backend"), allow(dead_code))]

use std::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::ringbuf::AudioRingConsumer;

/// Progress for the currently started output stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackProgress {
    /// Real samples copied from the TX ring into output callbacks since start().
    /// Callback silence is excluded. This count alone does not prove playback.
    pub consumed_samples: u64,
    /// Time remaining until the last consumed real sample is predicted to
    /// finish playing, measured when this snapshot is obtained.
    pub remaining: Duration,
}

impl PlaybackProgress {
    /// True once `target` real samples have been consumed and the last of
    /// them is predicted to have finished playing.
    ///
    /// `target` is the sum of `AudioSink::write` return counts on the
    /// currently started stream, never an assumed acceptance:
    ///
    /// ```
    /// use std::time::Duration;
    /// use coppa_audio::PlaybackProgress;
    ///
    /// let target = 65_520_u64;
    /// let draining = PlaybackProgress {
    ///     consumed_samples: 65_520,
    ///     remaining: Duration::from_millis(12),
    /// };
    /// assert!(!draining.has_played(target), "consumed is not played");
    /// let done = PlaybackProgress { remaining: Duration::ZERO, ..draining };
    /// assert!(done.has_played(target));
    /// ```
    pub fn has_played(&self, target: u64) -> bool {
        self.consumed_samples >= target && self.remaining.is_zero()
    }
}

/// Longest callback-to-playback delay accepted as a real device prediction.
/// Anything larger is treated as an invalid timestamp rather than trusted.
const MAX_PLAYBACK_DELAY: Duration = Duration::from_secs(10);

/// Shared state written by one output callback and read by the stream owner.
pub(crate) struct ProgressShared {
    /// Seqlock sequence: odd while the callback is publishing.
    seq: AtomicU64,
    consumed: AtomicU64,
    /// Predicted end of the last consumed real sample, in nanoseconds after
    /// `base`.
    deadline_nanos: AtomicU64,
    failed: AtomicBool,
    base: Instant,
}

impl ProgressShared {
    pub(crate) fn new(base: Instant) -> Self {
        Self {
            seq: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            deadline_nanos: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            base,
        }
    }

    /// Single-writer publish (only the output callback calls this).
    fn publish(&self, consumed: u64, deadline_nanos: u64) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.consumed.store(consumed, Ordering::Relaxed);
        self.deadline_nanos.store(deadline_nanos, Ordering::Relaxed);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    fn snapshot(&self) -> (u64, u64) {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let consumed = self.consumed.load(Ordering::Relaxed);
            let deadline = self.deadline_nanos.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return (consumed, deadline);
            }
            std::hint::spin_loop();
        }
    }

    /// Mark the stream failed (error callback or invalid timestamp).
    pub(crate) fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Progress snapshot as seen at `now`.
    pub(crate) fn progress_at(&self, now: Instant) -> Result<PlaybackProgress> {
        if self.is_failed() {
            return Err(anyhow!("output stream failed"));
        }
        let (consumed, deadline_nanos) = self.snapshot();
        let deadline = self.base + Duration::from_nanos(deadline_nanos);
        Ok(PlaybackProgress {
            consumed_samples: consumed,
            remaining: deadline.saturating_duration_since(now),
        })
    }
}

/// Per-stream state owned by the output callback itself.
pub(crate) struct CallbackState {
    consumed: u64,
    sample_rate: u32,
}

impl CallbackState {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            consumed: 0,
            sample_rate: sample_rate.max(1),
        }
    }
}

/// Nanoseconds `samples` take to play at `sample_rate`, rounded up so a
/// final fractional sample period is never dropped.
fn samples_to_nanos_ceil(samples: usize, sample_rate: u32) -> u128 {
    let rate = sample_rate as u128;
    (samples as u128 * 1_000_000_000).div_ceil(rate)
}

/// Fill one output callback buffer: copy real samples from the ring, pad the
/// rest with silence, and publish progress for the real prefix.
///
/// `entry` is a monotonic instant sampled at callback entry and
/// `playback_delay` is the device's predicted callback-to-playback delay
/// (`None` when the backend's timestamp is invalid, which fails the stream
/// rather than assuming zero latency). Silence-only callbacks publish nothing,
/// so the last real deadline survives them. Returns the real-sample count.
pub(crate) fn fill_output(
    consumer: &mut AudioRingConsumer,
    data: &mut [f32],
    shared: &ProgressShared,
    state: &mut CallbackState,
    entry: Instant,
    playback_delay: Option<Duration>,
) -> usize {
    if shared.is_failed() {
        data.fill(0.0);
        return 0;
    }
    let read = consumer.read(data);
    for sample in &mut data[read..] {
        *sample = 0.0;
    }
    if read == 0 {
        return 0;
    }
    let delay = match playback_delay {
        Some(d) if d <= MAX_PLAYBACK_DELAY => d,
        _ => {
            shared.fail();
            return read;
        }
    };
    state.consumed += read as u64;
    let start = entry.saturating_duration_since(shared.base).as_nanos() + delay.as_nanos();
    let end = start + samples_to_nanos_ceil(read, state.sample_rate);
    shared.publish(state.consumed, u64::try_from(end).unwrap_or(u64::MAX));
    read
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ringbuf::audio_ring;

    const SR: u32 = 48_000;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32).collect()
    }

    /// AC1: a 65,520-sample frame fed through an 8,192-sample ring by short,
    /// retained-suffix writes arrives at the callback complete and in order,
    /// with zero overflow.
    #[test]
    fn full_frame_through_small_ring_arrives_in_order() {
        let frame = ramp(65_520);
        let (mut prod, mut cons) = audio_ring(8192);
        let base = Instant::now();
        let shared = ProgressShared::new(base);
        let mut state = CallbackState::new(SR);
        let mut captured = Vec::with_capacity(frame.len());
        let mut offset = 0;
        let callback_sizes = [480usize, 1024, 333, 4096, 1];
        let mut i = 0;
        while captured.len() < frame.len() {
            let room = prod.available();
            let end = (offset + room).min(frame.len());
            offset += prod.write(&frame[offset..end]);
            let mut buf = vec![0.0; callback_sizes[i % callback_sizes.len()]];
            let read = fill_output(
                &mut cons,
                &mut buf,
                &shared,
                &mut state,
                base,
                Some(Duration::ZERO),
            );
            captured.extend_from_slice(&buf[..read]);
            i += 1;
        }
        assert_eq!(captured, frame);
        assert_eq!(prod.overflow_count(), 0);
        assert_eq!(shared.progress_at(base).unwrap().consumed_samples, 65_520);
    }

    /// AC1: while the consumer is held, writes return zero; resuming neither
    /// skips nor duplicates the retained suffix.
    #[test]
    fn held_consumer_then_resume_keeps_suffix() {
        let frame = ramp(20_000);
        let (mut prod, mut cons) = audio_ring(8192);
        let base = Instant::now();
        let shared = ProgressShared::new(base);
        let mut state = CallbackState::new(SR);
        let mut offset = prod.write(&frame[..8192]);
        assert_eq!(offset, 8192);
        // Held: no room, so a capped write offers nothing and accepts nothing.
        let room = prod.available();
        assert_eq!(room, 0);
        assert_eq!(prod.write(&frame[offset..offset + room]), 0);
        let mut captured = Vec::new();
        while captured.len() < frame.len() {
            let mut buf = vec![0.0; 1500];
            let read = fill_output(
                &mut cons,
                &mut buf,
                &shared,
                &mut state,
                base,
                Some(Duration::ZERO),
            );
            captured.extend_from_slice(&buf[..read]);
            let end = (offset + prod.available()).min(frame.len());
            offset += prod.write(&frame[offset..end]);
        }
        assert_eq!(captured, frame);
        assert_eq!(prod.overflow_count(), 0);
    }

    /// Final partial callback then silence: count excludes padding and the
    /// last real deadline survives silent callbacks.
    #[test]
    fn partial_callback_then_silence_keeps_last_deadline() {
        let (mut prod, mut cons) = audio_ring(8192);
        let base = Instant::now();
        let shared = ProgressShared::new(base);
        let mut state = CallbackState::new(SR);
        prod.write(&[0.5; 100]);
        let mut buf = vec![9.0; 480];
        let read = fill_output(
            &mut cons,
            &mut buf,
            &shared,
            &mut state,
            base,
            Some(Duration::from_millis(20)),
        );
        assert_eq!(read, 100);
        assert!(buf[..100].iter().all(|&s| s == 0.5));
        assert!(buf[100..].iter().all(|&s| s == 0.0));
        let first = shared.progress_at(base).unwrap();
        assert_eq!(first.consumed_samples, 100);
        // 20 ms delay + ceil(100 / 48 kHz) = 20 ms + 2_083_334 ns.
        assert_eq!(
            first.remaining,
            Duration::from_nanos(20_000_000 + 2_083_334)
        );

        let later = base + Duration::from_millis(10);
        let read = fill_output(
            &mut cons,
            &mut buf,
            &shared,
            &mut state,
            later,
            Some(Duration::from_millis(20)),
        );
        assert_eq!(read, 0);
        assert!(buf.iter().all(|&s| s == 0.0));
        let after = shared.progress_at(base).unwrap();
        assert_eq!(after, first, "silence must not move count or deadline");
    }

    /// Completion stays false until the predicted presentation deadline,
    /// including a sub-millisecond final sample period, and is not implied by
    /// the ring being empty or the samples having been consumed.
    #[test]
    fn completion_waits_for_presentation_deadline() {
        let (mut prod, mut cons) = audio_ring(8192);
        let base = Instant::now();
        let shared = ProgressShared::new(base);
        let mut state = CallbackState::new(SR);
        prod.write(&[0.1; 7]);
        let mut buf = vec![0.0; 64];
        fill_output(
            &mut cons,
            &mut buf,
            &shared,
            &mut state,
            base,
            Some(Duration::from_millis(30)),
        );
        assert_eq!(cons.available(), 0, "ring is empty");
        let consumed = shared.progress_at(base).unwrap();
        assert_eq!(consumed.consumed_samples, 7);
        assert!(!consumed.has_played(7), "consumed is not played");
        // 7 samples at 48 kHz = 145_834 ns (rounded up).
        let deadline = base + Duration::from_millis(30) + Duration::from_nanos(145_834);
        let just_before = deadline - Duration::from_nanos(1);
        assert!(!shared.progress_at(just_before).unwrap().has_played(7));
        assert!(shared.progress_at(deadline).unwrap().has_played(7));
        assert!(!shared.progress_at(deadline).unwrap().has_played(8));
    }

    #[test]
    fn invalid_timestamp_fails_the_stream() {
        let (mut prod, mut cons) = audio_ring(64);
        let base = Instant::now();
        let shared = ProgressShared::new(base);
        let mut state = CallbackState::new(SR);
        prod.write(&[0.2; 10]);
        let mut buf = vec![0.0; 10];
        fill_output(&mut cons, &mut buf, &shared, &mut state, base, None);
        assert!(shared.progress_at(base).is_err());
        // A failed stream outputs silence and reports no further progress.
        prod.write(&[0.2; 10]);
        let mut buf = vec![1.0; 10];
        assert_eq!(
            fill_output(
                &mut cons,
                &mut buf,
                &shared,
                &mut state,
                base,
                Some(Duration::ZERO)
            ),
            0
        );
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn absurd_delay_is_invalid() {
        let (mut prod, mut cons) = audio_ring(64);
        let base = Instant::now();
        let shared = ProgressShared::new(base);
        let mut state = CallbackState::new(SR);
        prod.write(&[0.2; 10]);
        let mut buf = vec![0.0; 10];
        fill_output(
            &mut cons,
            &mut buf,
            &shared,
            &mut state,
            base,
            Some(Duration::from_secs(60)),
        );
        assert!(shared.is_failed());
    }

    #[test]
    fn error_flag_fails_progress() {
        let shared = ProgressShared::new(Instant::now());
        assert!(shared.progress_at(Instant::now()).is_ok());
        shared.fail();
        assert!(shared.progress_at(Instant::now()).is_err());
    }

    /// Concurrent publication: a reader never observes a count paired with a
    /// deadline from a different publish.
    #[test]
    fn snapshots_are_coherent_under_concurrency() {
        use std::sync::Arc;
        let shared = Arc::new(ProgressShared::new(Instant::now()));
        let writer = {
            let shared = shared.clone();
            std::thread::spawn(move || {
                for i in 1..=200_000u64 {
                    // Invariant every publish holds: deadline == 3 * count.
                    shared.publish(i, i * 3);
                }
            })
        };
        let mut checks = 0u64;
        while !writer.is_finished() || checks == 0 {
            let (c, d) = shared.snapshot();
            assert_eq!(d, c * 3, "torn snapshot: count {c} deadline {d}");
            checks += 1;
        }
        writer.join().unwrap();
        assert_eq!(shared.snapshot(), (200_000, 600_000));
    }
}
