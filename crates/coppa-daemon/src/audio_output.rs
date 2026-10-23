//! Lossless TX audio delivery shared by the daemon event loop and the TNC.
//!
//! A transmission passes through two bounded rings before it reaches the
//! device: the daemon's outer audio-out ring, then the sink's own TX ring
//! (8,192 samples for `CpalSink`). A frame is routinely larger than either
//! (a level-2 OFDM frame is 65,520 samples; a 256-byte AFSK frame is
//! 94,080-110,440), so every producer boundary here retains its unwritten
//! suffix and retries it instead of dropping it:
//!
//! - [`PendingOutput`] holds samples ahead of the outer ring and offers at
//!   most the ring's free space each tick.
//! - [`OutputWorker`] forwards the outer ring into the final sink one chunk
//!   at a time and never reads a new chunk until the sink accepted the whole
//!   previous one.
//! - [`OutputFeedback`] carries the sink's playback progress back to the
//!   caller as a single "played through sample N" counter, and
//!   [`PlayoutWatch`] turns it into completion, a no-progress stall, or a
//!   watchdog expiry, so PTT is released from samples actually played rather
//!   than from a nominal duration.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use coppa_audio::{AudioRingConsumer, AudioRingProducer, AudioSink, PlaybackProgress};

/// Longest gap between TX delivery opportunities (pending flush, completion
/// checks). Far inside the 8,192-sample ring's ~171 ms at 48 kHz.
pub const PROGRESS_TICK: Duration = Duration::from_millis(5);

/// Time without downstream playback progress after which an active
/// transmission is failed rather than held keyed indefinitely.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Samples retained ahead of the outer audio-out ring, in submission order.
#[derive(Debug, Default)]
pub struct PendingOutput {
    queue: VecDeque<f32>,
    /// Cumulative samples submitted that will reach the outer ring (cleared
    /// samples are subtracted, so this stays aligned with what the sink can
    /// ever consume).
    submitted: u64,
}

impl PendingOutput {
    /// Queue `samples` behind everything already pending. Returns the
    /// cumulative sample count that must play before these have all played.
    pub fn push(&mut self, samples: &[f32]) -> u64 {
        self.queue.extend(samples.iter().copied());
        self.submitted += samples.len() as u64;
        self.submitted
    }

    /// Move as many pending samples as the ring has room for. Never offers
    /// more than `available()`, so backpressure is never counted as overflow.
    pub fn flush(&mut self, producer: &mut AudioRingProducer) -> usize {
        let mut moved = 0;
        while !self.queue.is_empty() {
            let room = producer.available();
            if room == 0 {
                break;
            }
            let (front, _) = self.queue.as_slices();
            let take = front.len().min(room);
            let written = producer.write(&front[..take]);
            self.queue.drain(..written);
            moved += written;
            if written < take {
                break;
            }
        }
        moved
    }

    /// Samples not yet accepted by the outer ring.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// True when nothing is waiting for the outer ring.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Cumulative samples submitted (see the field doc).
    pub fn submitted(&self) -> u64 {
        self.submitted
    }

    /// Discard every pending sample (a cancelled transmission must not play
    /// later), keeping `submitted` aligned with what can still be consumed.
    pub fn clear(&mut self) -> usize {
        let dropped = self.queue.len();
        self.queue.clear();
        self.submitted -= dropped as u64;
        dropped
    }
}

/// Playback progress published by the output worker for the TX owner.
#[derive(Debug, Default)]
pub struct OutputFeedback {
    /// Samples (cumulative since the stream started) predicted to have
    /// finished playing.
    played: AtomicU64,
    failed: AtomicBool,
    /// Cumulative sample index below which samples not yet handed to the
    /// sink must be discarded (set when the TX owner cancels).
    discard_through: AtomicU64,
}

impl OutputFeedback {
    /// Samples predicted to have finished playing.
    pub fn played(&self) -> u64 {
        self.played.load(Ordering::Acquire)
    }

    /// True once the output stream has failed; it stays failed.
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Mark the output failed.
    pub fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }

    /// Ask the output worker to drop every sample with a cumulative index
    /// below `through` that it has not yet handed to the sink: the outer
    /// ring's contents and its retained chunk. Used when a transmission is
    /// cancelled so its audio cannot play after PTT is released.
    pub fn discard_through(&self, through: u64) {
        self.discard_through.fetch_max(through, Ordering::AcqRel);
    }

    /// Publish a sink progress snapshot as one coherent value: consumed
    /// samples minus those whose predicted playback is still ahead (rounded
    /// up, so a partly-played sample is not counted as played), plus the
    /// `discarded` samples that will never reach the sink, so cumulative
    /// targets stay aligned after a cancel.
    pub fn publish(&self, progress: PlaybackProgress, sample_rate: u32, discarded: u64) {
        let still_playing =
            (progress.remaining.as_nanos() * sample_rate as u128).div_ceil(1_000_000_000);
        let played = progress
            .consumed_samples
            .saturating_sub(u64::try_from(still_playing).unwrap_or(u64::MAX))
            .saturating_add(discarded);
        self.played.fetch_max(played, Ordering::AcqRel);
    }
}

/// Forwards the outer audio-out ring into the final sink, retaining a chunk
/// until the sink has accepted all of it.
pub struct OutputWorker {
    buf: Vec<f32>,
    len: usize,
    offset: usize,
    /// Cumulative samples read from the outer ring.
    read_total: u64,
    /// Cumulative samples dropped by `discard_through` requests.
    discarded: u64,
    feedback: Arc<OutputFeedback>,
    sample_rate: u32,
}

impl OutputWorker {
    /// `chunk` is the most samples read from the outer ring at once.
    pub fn new(chunk: usize, sample_rate: u32, feedback: Arc<OutputFeedback>) -> Self {
        Self {
            buf: vec![0.0; chunk.max(1)],
            len: 0,
            offset: 0,
            read_total: 0,
            discarded: 0,
            feedback,
            sample_rate,
        }
    }

    /// One forwarding step: offer the retained chunk's remainder (or a fresh
    /// chunk once it is fully accepted) to `sink`, then publish `progress`.
    /// Returns how many samples the sink accepted. A sink or progress error
    /// fails the feedback and is returned.
    pub fn step<S: AudioSink>(
        &mut self,
        outer: &mut AudioRingConsumer,
        sink: &mut S,
        progress: impl FnOnce(&S) -> Result<PlaybackProgress>,
    ) -> Result<usize> {
        let result = self.forward(outer, sink).and_then(|accepted| {
            let snapshot = progress(sink)?;
            self.feedback
                .publish(snapshot, self.sample_rate, self.discarded);
            Ok(accepted)
        });
        if result.is_err() {
            self.feedback.fail();
        }
        result
    }

    fn forward<S: AudioSink>(
        &mut self,
        outer: &mut AudioRingConsumer,
        sink: &mut S,
    ) -> Result<usize> {
        self.apply_discard(outer);
        if self.offset == self.len {
            self.len = outer.read(&mut self.buf);
            self.offset = 0;
            self.read_total += self.len as u64;
            if self.len == 0 {
                return Ok(0);
            }
        }
        let accepted = sink.write(&self.buf[self.offset..self.len])?;
        self.offset += accepted.min(self.len - self.offset);
        Ok(accepted)
    }

    /// Drop not-yet-forwarded samples below the requested discard index.
    fn apply_discard(&mut self, outer: &mut AudioRingConsumer) {
        let through = self.feedback.discard_through.load(Ordering::Acquire);
        // Index of the next sample that would be handed to the sink.
        let next = self.read_total - (self.len - self.offset) as u64;
        if through <= next {
            return;
        }
        self.discarded += (self.len - self.offset) as u64;
        self.offset = self.len;
        while self.read_total < through {
            let want = (through - self.read_total).min(self.buf.len() as u64) as usize;
            let n = outer.read(&mut self.buf[..want]);
            if n == 0 {
                break;
            }
            self.read_total += n as u64;
            self.discarded += n as u64;
        }
        self.len = 0;
        self.offset = 0;
    }

    /// Run until shutdown, the outer ring's producer is gone, or the output
    /// fails, sleeping briefly whenever nothing moved. Used by the daemon
    /// and TNC output threads, which own (create, start, stop and drop) the
    /// sink on that same thread.
    pub fn run<S: AudioSink>(
        mut self,
        mut outer: AudioRingConsumer,
        mut sink: S,
        progress: impl Fn(&S) -> Result<PlaybackProgress>,
        shutdown: Arc<AtomicBool>,
    ) {
        loop {
            if shutdown.load(Ordering::Acquire) || outer.is_abandoned() {
                break;
            }
            match self.step(&mut outer, &mut sink, &progress) {
                Ok(0) => std::thread::sleep(Duration::from_millis(2)),
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(error = %e, "Audio output failed; TX output stopped");
                    break;
                }
            }
        }
        if let Err(e) = sink.stop() {
            tracing::warn!(error = %e, "Failed to stop audio output");
        }
    }
}

/// Why an active transmission ended without completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayoutFailure {
    /// The output stream reported failure.
    OutputFailed,
    /// No playback progress for [`STALL_TIMEOUT`].
    Stalled,
    /// The transmission exceeded its maximum duration.
    Watchdog,
}

impl std::fmt::Display for PlayoutFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PlayoutFailure::OutputFailed => "audio output failed",
            PlayoutFailure::Stalled => "audio output made no progress",
            PlayoutFailure::Watchdog => "transmission exceeded its maximum duration",
        })
    }
}

/// Outcome of one [`PlayoutWatch::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayoutState {
    /// Still playing.
    Pending,
    /// Every target sample is predicted to have finished playing.
    Played,
    /// The transmission must be cancelled.
    Failed(PlayoutFailure),
}

/// Tracks one transmission's cumulative playback target.
#[derive(Debug, Clone)]
pub struct PlayoutWatch {
    target: u64,
    started: Instant,
    max_duration: Option<Duration>,
    last_played: u64,
    last_progress: Instant,
}

impl PlayoutWatch {
    /// Watch for `target` cumulative samples to play, starting at `now`.
    pub fn new(target: u64, now: Instant, max_duration: Option<Duration>) -> Self {
        Self {
            target,
            started: now,
            max_duration,
            last_played: 0,
            last_progress: now,
        }
    }

    /// Cumulative target this watch completes at.
    pub fn target(&self) -> u64 {
        self.target
    }

    /// Advance with the latest feedback. Only downstream playback progress
    /// resets the stall timer; enqueueing does not.
    pub fn poll(&mut self, played: u64, failed: bool, now: Instant) -> PlayoutState {
        if failed {
            return PlayoutState::Failed(PlayoutFailure::OutputFailed);
        }
        if played >= self.target {
            return PlayoutState::Played;
        }
        if played > self.last_played {
            self.last_played = played;
            self.last_progress = now;
        }
        if let Some(max) = self.max_duration {
            if now.saturating_duration_since(self.started) >= max {
                return PlayoutState::Failed(PlayoutFailure::Watchdog);
            }
        }
        if now.saturating_duration_since(self.last_progress) >= STALL_TIMEOUT {
            return PlayoutState::Failed(PlayoutFailure::Stalled);
        }
        PlayoutState::Pending
    }
}

/// Wait (on the Tokio clock) until `watch` completes or fails, checking the
/// feedback every [`PROGRESS_TICK`].
pub async fn wait_for_playout(
    feedback: &OutputFeedback,
    mut watch: PlayoutWatch,
) -> std::result::Result<(), PlayoutFailure> {
    let start_tokio = tokio::time::Instant::now();
    let start_std = Instant::now();
    loop {
        // Map Tokio's (possibly paused) clock onto the watch's std clock so
        // tests can drive stalls and watchdogs deterministically.
        let now = start_std + start_tokio.elapsed();
        match watch.poll(feedback.played(), feedback.is_failed(), now) {
            PlayoutState::Played => return Ok(()),
            PlayoutState::Failed(why) => return Err(why),
            PlayoutState::Pending => tokio::time::sleep(PROGRESS_TICK).await,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use coppa_audio::audio_ring;

    /// Simulated final sink: a bounded ring like `CpalSink`'s, drained by a
    /// test-controlled "callback", with a configurable presentation delay.
    pub struct SimSink {
        producer: AudioRingProducer,
        consumer: AudioRingConsumer,
        pub captured: Vec<f32>,
        /// Scripted per-call write limits (None = unlimited), cycled.
        pub write_limits: Vec<Option<usize>>,
        calls: usize,
        pub fail_writes: bool,
        /// Remaining presentation time reported for the last consumed sample.
        pub remaining: Duration,
    }

    impl SimSink {
        pub fn new(capacity: usize) -> Self {
            let (producer, consumer) = audio_ring(capacity);
            Self {
                producer,
                consumer,
                captured: Vec::new(),
                write_limits: vec![None],
                calls: 0,
                fail_writes: false,
                remaining: Duration::ZERO,
            }
        }

        /// Simulated device callback consuming up to `n` real samples.
        pub fn callback(&mut self, n: usize) -> usize {
            let mut buf = vec![0.0; n];
            let read = self.consumer.read(&mut buf);
            self.captured.extend_from_slice(&buf[..read]);
            read
        }

        pub fn progress(&self) -> Result<PlaybackProgress> {
            Ok(PlaybackProgress {
                consumed_samples: self.captured.len() as u64,
                remaining: self.remaining,
            })
        }
    }

    impl AudioSink for SimSink {
        fn write(&mut self, samples: &[f32]) -> Result<usize> {
            if self.fail_writes {
                anyhow::bail!("simulated output failure");
            }
            let limit = self.write_limits[self.calls % self.write_limits.len()];
            self.calls += 1;
            let take = samples
                .len()
                .min(self.producer.available())
                .min(limit.unwrap_or(usize::MAX));
            Ok(self.producer.write(&samples[..take]))
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// Progress closure for [`OutputWorker::step`] over a [`SimSink`].
    pub fn sim_progress(sink: &SimSink) -> Result<PlaybackProgress> {
        sink.progress()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use coppa_audio::audio_ring;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i % 100_003) as f32 + 0.5).collect()
    }

    /// Drive pending -> outer ring -> worker -> sim sink -> callback until
    /// everything has been captured.
    fn deliver(
        frame: &[f32],
        outer_cap: usize,
        sink: &mut SimSink,
        callback_sizes: &[usize],
    ) -> (Vec<f32>, u64) {
        let (mut prod, mut cons) = audio_ring(outer_cap);
        let feedback = Arc::new(OutputFeedback::default());
        let mut worker = OutputWorker::new(1024, 48_000, feedback.clone());
        let mut pending = PendingOutput::default();
        let target = pending.push(frame);
        let mut i = 0;
        while feedback.played() < target {
            pending.flush(&mut prod);
            for _ in 0..3 {
                worker.step(&mut cons, sink, sim_progress).unwrap();
            }
            sink.callback(callback_sizes[i % callback_sizes.len()]);
            i += 1;
            assert!(i < 1_000_000, "delivery did not converge");
        }
        assert_eq!(
            prod.overflow_count(),
            0,
            "retained samples are never dropped"
        );
        (sink.captured.clone(), feedback.played())
    }

    /// AC1: a 65,520-sample frame crosses an 8,192 outer ring and an 8,192
    /// sink ring with uneven, delayed consumption, arriving complete.
    #[test]
    fn two_ring_delivery_is_lossless() {
        let frame = ramp(65_520);
        let mut sink = SimSink::new(8192);
        let (captured, played) = deliver(&frame, 8192, &mut sink, &[480, 64, 4096, 0, 0, 1000]);
        assert_eq!(captured, frame);
        assert_eq!(played, 65_520);
    }

    /// Short and zero sink writes: the worker finishes its current chunk
    /// (including a final chunk smaller than 1,024) before reading another.
    #[test]
    fn short_and_zero_writes_keep_chunk_order() {
        let frame = ramp(10_000 + 517);
        let mut sink = SimSink::new(8192);
        sink.write_limits = vec![Some(100), Some(0), Some(0), Some(333), None, Some(1)];
        let (captured, _) = deliver(&frame, 2048, &mut sink, &[200, 0, 777]);
        assert_eq!(captured, frame);
    }

    #[test]
    fn worker_step_publishes_progress_and_fails_on_sink_error() {
        let (mut prod, mut cons) = audio_ring(4096);
        let feedback = Arc::new(OutputFeedback::default());
        let mut worker = OutputWorker::new(1024, 48_000, feedback.clone());
        let mut sink = SimSink::new(8192);
        prod.write(&[0.25; 3000]);
        assert_eq!(
            worker.step(&mut cons, &mut sink, sim_progress).unwrap(),
            1024
        );
        sink.fail_writes = true;
        assert!(worker.step(&mut cons, &mut sink, sim_progress).is_err());
        assert!(feedback.is_failed());
    }

    #[test]
    fn feedback_subtracts_samples_still_playing() {
        let fb = OutputFeedback::default();
        fb.publish(
            PlaybackProgress {
                consumed_samples: 10_000,
                remaining: Duration::from_millis(100),
            },
            48_000,
            0,
        );
        assert_eq!(fb.played(), 10_000 - 4_800);
        fb.publish(
            PlaybackProgress {
                consumed_samples: 10_000,
                remaining: Duration::ZERO,
            },
            48_000,
            0,
        );
        assert_eq!(fb.played(), 10_000);
        // A fractional sample still ahead keeps the last sample unplayed.
        let fb = OutputFeedback::default();
        fb.publish(
            PlaybackProgress {
                consumed_samples: 7,
                remaining: Duration::from_nanos(1),
            },
            48_000,
            0,
        );
        assert_eq!(fb.played(), 6);
    }

    /// A cancel discards the outer ring's contents and the retained chunk
    /// (never the sink's own ring), and later audio still completes against
    /// its cumulative target.
    #[test]
    fn discard_drops_unforwarded_samples_and_keeps_targets_aligned() {
        let (mut prod, mut cons) = audio_ring(8192);
        let feedback = Arc::new(OutputFeedback::default());
        let mut worker = OutputWorker::new(1024, 48_000, feedback.clone());
        let mut sink = SimSink::new(2048);
        let mut pending = PendingOutput::default();
        let cancelled: Vec<f32> = vec![1.0; 20_000];
        pending.push(&cancelled);
        pending.flush(&mut prod);
        for _ in 0..4 {
            worker.step(&mut cons, &mut sink, sim_progress).unwrap();
        }
        // Cancel: the owner drops its FIFO and asks the worker to discard.
        pending.clear();
        feedback.discard_through(pending.submitted());
        let next: Vec<f32> = (0..5_000).map(|i| i as f32 + 2.0).collect();
        let target = pending.push(&next);
        while feedback.played() < target {
            pending.flush(&mut prod);
            worker.step(&mut cons, &mut sink, sim_progress).unwrap();
            sink.callback(700);
        }
        let captured = &sink.captured;
        // Only what was already inside the sink's ring of the cancelled job
        // can still play; then the next job, whole and in order.
        assert!(captured.len() <= 2048 + next.len());
        assert_eq!(&captured[captured.len() - next.len()..], &next[..]);
        assert!(captured[..captured.len() - next.len()]
            .iter()
            .all(|&s| s == 1.0));
    }

    #[test]
    fn pending_clear_keeps_targets_aligned() {
        let (mut prod, _cons) = audio_ring(100);
        let mut pending = PendingOutput::default();
        assert_eq!(pending.push(&[1.0; 250]), 250);
        assert_eq!(pending.flush(&mut prod), 100);
        assert_eq!(pending.clear(), 150);
        assert_eq!(pending.submitted(), 100);
        assert_eq!(pending.push(&[2.0; 10]), 110);
        assert_eq!(prod.overflow_count(), 0);
    }

    #[test]
    fn watch_completes_only_when_target_played() {
        let t0 = Instant::now();
        let mut w = PlayoutWatch::new(1000, t0, None);
        assert_eq!(w.poll(999, false, t0), PlayoutState::Pending);
        assert_eq!(w.poll(1000, false, t0), PlayoutState::Played);
    }

    #[test]
    fn watch_stalls_after_five_seconds_without_progress() {
        let t0 = Instant::now();
        let mut w = PlayoutWatch::new(1000, t0, None);
        assert_eq!(
            w.poll(10, false, t0 + Duration::from_secs(4)),
            PlayoutState::Pending
        );
        // Progress at 4 s resets the stall timer.
        assert_eq!(
            w.poll(10, false, t0 + Duration::from_millis(8_999)),
            PlayoutState::Pending
        );
        assert_eq!(
            w.poll(10, false, t0 + Duration::from_secs(9)),
            PlayoutState::Failed(PlayoutFailure::Stalled)
        );
    }

    #[test]
    fn watch_enforces_max_duration_and_output_failure() {
        let t0 = Instant::now();
        let mut w = PlayoutWatch::new(u64::MAX, t0, Some(Duration::from_secs(30)));
        for s in 1..30u64 {
            assert_eq!(
                w.poll(s * 100, false, t0 + Duration::from_secs(s)),
                PlayoutState::Pending
            );
        }
        assert_eq!(
            w.poll(3_100, false, t0 + Duration::from_secs(30)),
            PlayoutState::Failed(PlayoutFailure::Watchdog)
        );
        let mut w = PlayoutWatch::new(10, t0, None);
        assert_eq!(
            w.poll(0, true, t0),
            PlayoutState::Failed(PlayoutFailure::OutputFailed)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_playout_reports_stall_on_paused_clock() {
        let fb = OutputFeedback::default();
        let watch = PlayoutWatch::new(10, Instant::now(), None);
        let started = tokio::time::Instant::now();
        assert_eq!(
            wait_for_playout(&fb, watch).await,
            Err(PlayoutFailure::Stalled)
        );
        assert!(started.elapsed() >= STALL_TIMEOUT);
    }
}
