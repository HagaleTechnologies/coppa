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

/// Retained TX samples plus the outer ring they feed. Shared (behind a
/// mutex held only briefly, never across an await) between the daemon event
/// loop, which queues and cancels transmissions, and a feeder task that moves
/// samples into the ring every [`PROGRESS_TICK`] whatever the event loop is
/// awaiting, so a frame still playing is never starved mid-transmission.
#[derive(Default)]
pub struct TxOutput {
    pending: PendingOutput,
    producer: Option<AudioRingProducer>,
}

impl TxOutput {
    /// Wire the outer audio-out ring.
    pub fn set_producer(&mut self, producer: AudioRingProducer) {
        self.producer = Some(producer);
    }

    /// True when a ring is wired and something still consumes it.
    pub fn is_live(&self) -> bool {
        self.producer.as_ref().is_some_and(|p| !p.is_abandoned())
    }

    /// Queue `samples` behind everything pending and move what fits. Returns
    /// the cumulative playback target, or `None` (nothing queued) when no
    /// live ring is wired, so undeliverable audio cannot accumulate.
    pub fn submit(&mut self, samples: &[f32]) -> Option<u64> {
        if !self.is_live() {
            return None;
        }
        let target = self.pending.push(samples);
        self.flush();
        Some(target)
    }

    /// Register the transmission boundary before exposing its samples to the
    /// output worker. This prevents a fast worker from crossing the boundary
    /// before its finish marker becomes visible.
    pub fn submit_with_finish(
        &mut self,
        samples: &[f32],
        feedback: &OutputFeedback,
    ) -> Option<u64> {
        if !self.is_live() {
            return None;
        }
        let target = self.pending.push(samples);
        feedback.finish_through(target);
        self.flush();
        Some(target)
    }

    /// Move retained samples into the ring as far as it has room.
    pub fn flush(&mut self) -> usize {
        match self.producer.as_mut() {
            Some(producer) => self.pending.flush(producer),
            None => 0,
        }
    }

    /// Discard every retained sample; returns how many were dropped.
    pub fn cancel(&mut self) -> usize {
        self.pending.clear()
    }

    /// Cumulative samples submitted that can still reach the ring.
    pub fn submitted(&self) -> u64 {
        self.pending.submitted()
    }

    /// Samples not yet accepted by the ring.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The ring's overflow counter (always zero for paced TX).
    pub fn overflow_count(&self) -> u64 {
        self.producer.as_ref().map_or(0, |p| p.overflow_count())
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
    /// Completed transmission boundaries awaiting the sink's tail flush.
    finish_targets: std::sync::Mutex<VecDeque<u64>>,
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
    /// below `through` that has not yet played: the outer ring's contents,
    /// its retained chunk, and what the final sink still buffers (see
    /// [`AudioSink::discard_buffered`]). Used when a transmission is
    /// cancelled so its audio cannot play after PTT is released, nor at the
    /// start of the next transmission. Once the drop has taken effect and
    /// anything the device already held has played, `played()` reaches
    /// `through` (see [`wait_for_discard`]).
    pub fn discard_through(&self, through: u64) {
        self.discard_through.fetch_max(through, Ordering::AcqRel);
        self.finish_targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Publish one complete transmission's cumulative mono-sample target.
    /// The worker flushes the final partial conversion block only after all
    /// samples through this boundary have reached the sink.
    pub fn finish_through(&self, target: u64) {
        self.finish_targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(target);
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
    /// Highest `discard_through` request already acted on.
    applied_through: u64,
    /// The sink has been asked to drop its buffer and has not confirmed;
    /// nothing more is written to it until it does.
    sink_flush_pending: bool,
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
            applied_through: 0,
            sink_flush_pending: false,
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
            self.finish_completed_tails(sink)?;
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

    fn finish_completed_tails<S: AudioSink>(&mut self, sink: &mut S) -> Result<()> {
        let forwarded = self.read_total - (self.len - self.offset) as u64;
        loop {
            let target = {
                let mut targets = self
                    .feedback
                    .finish_targets
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if targets.front().is_some_and(|target| *target <= forwarded) {
                    targets.pop_front()
                } else {
                    None
                }
            };
            if target.is_none() {
                break;
            }
            sink.finish_input()?;
        }
        Ok(())
    }

    fn forward<S: AudioSink>(
        &mut self,
        outer: &mut AudioRingConsumer,
        sink: &mut S,
    ) -> Result<usize> {
        self.apply_discard(outer);
        if self.sink_flush_pending {
            match sink.discard_buffered()? {
                Some(dropped) => {
                    self.discarded += dropped;
                    self.sink_flush_pending = false;
                }
                // Writing now would put new audio behind the cancelled
                // tail, where the deferred drop could take it too.
                None => return Ok(0),
            }
        }
        if self.offset == self.len {
            self.len = outer.read(&mut self.buf);
            self.offset = 0;
            self.read_total += self.len as u64;
            if self.len == 0 {
                return Ok(0);
            }
        }
        let forwarded = self.read_total - (self.len - self.offset) as u64;
        let boundary = self
            .feedback
            .finish_targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .front()
            .copied();
        let remaining = boundary.map_or(self.len - self.offset, |target| {
            target
                .saturating_sub(forwarded)
                .min((self.len - self.offset) as u64) as usize
        });
        if remaining == 0 {
            return Ok(0);
        }
        let accepted = sink.write(&self.buf[self.offset..self.offset + remaining])?;
        self.offset += accepted.min(self.len - self.offset);
        Ok(accepted)
    }

    /// Act on a new discard request: drop not-yet-forwarded samples below
    /// the requested index, and ask the sink to drop what it still buffers.
    fn apply_discard(&mut self, outer: &mut AudioRingConsumer) {
        let through = self.feedback.discard_through.load(Ordering::Acquire);
        if through <= self.applied_through {
            return;
        }
        self.applied_through = through;
        // Index of the next sample that would be handed to the sink.
        let next = self.read_total - (self.len - self.offset) as u64;
        if through < next {
            // Samples past the cancelled range already reached the sink, so
            // its buffer is not all cancelled audio: leave it alone.
            return;
        }
        // Everything handed to the sink so far is cancelled audio.
        self.sink_flush_pending = true;
        if through == next {
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

    /// Continue an earlier no-progress window instead of starting a fresh
    /// one: the last observed playback count and when it last advanced.
    pub fn resume_progress(&mut self, last_played: u64, last_progress: Instant) {
        self.last_played = last_played;
        self.last_progress = last_progress;
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

/// Playback-progress record shared by every transmission in one continuous
/// keyed interval, so the [`STALL_TIMEOUT`] no-progress bound spans the
/// interval rather than restarting with each queued job. Times are on the
/// Tokio clock so watches started at different moments agree on them.
#[derive(Debug)]
pub struct StallClock {
    state: std::sync::Mutex<(u64, tokio::time::Instant)>,
}

impl StallClock {
    /// A fresh window: `played` samples so far, counted from `now`.
    pub fn new(played: u64, now: tokio::time::Instant) -> Self {
        Self {
            state: std::sync::Mutex::new((played, now)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (u64, tokio::time::Instant)> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Last observed playback count and when it last advanced (or when
    /// nothing was outstanding).
    pub fn snapshot(&self) -> (u64, tokio::time::Instant) {
        *self.lock()
    }

    /// Record an observation: the window restarts when playback advanced,
    /// or when nothing is outstanding (`idle`), since waiting on nothing is
    /// not a stall.
    pub fn observe(&self, played: u64, idle: bool, now: tokio::time::Instant) {
        let mut state = self.lock();
        if played > state.0 || idle {
            state.0 = state.0.max(played);
            state.1 = now;
        }
    }
}

/// Where a watch's no-progress window continues from: the keyed interval's
/// [`StallClock`], and the cumulative sample count queued before this
/// watch's job. If earlier audio is still outstanding when the watch starts,
/// it inherits the interval's window; otherwise it starts a fresh one.
#[derive(Debug, Clone, Copy)]
pub struct IntervalProgress<'a> {
    pub clock: &'a StallClock,
    pub prior_target: u64,
}

/// Longest wait, after a cancel, for the cancelled audio to be dropped from
/// every buffer (or finish playing from the device's own) before the next
/// transmission may key. Far beyond a healthy device's latency; reached only
/// when the device has stopped consuming, in which case the output worker
/// still holds new audio back until the sink confirms the drop.
pub const DISCARD_SETTLE_TIMEOUT: Duration = Duration::from_millis(500);

/// After `feedback.discard_through(through)`, wait (on the Tokio clock)
/// until every sample below `through` has been dropped or has played, the
/// output has failed, or [`DISCARD_SETTLE_TIMEOUT`] passes. Returns true
/// when the cancelled audio is gone.
pub async fn wait_for_discard(feedback: &OutputFeedback, through: u64) -> bool {
    let deadline = tokio::time::Instant::now() + DISCARD_SETTLE_TIMEOUT;
    loop {
        if discard_settled(feedback, through) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(PROGRESS_TICK).await;
    }
}

/// Non-blocking form of [`wait_for_discard`]'s condition, for the TNC's
/// polled state machine.
pub fn discard_settled(feedback: &OutputFeedback, through: u64) -> bool {
    feedback.played() >= through || feedback.is_failed()
}

/// Wait (on the Tokio clock) until `watch` completes or fails, checking the
/// feedback every [`PROGRESS_TICK`]. With `interval`, the no-progress bound
/// is shared across the keyed interval's transmissions (see [`StallClock`]).
pub async fn wait_for_playout(
    feedback: &OutputFeedback,
    mut watch: PlayoutWatch,
    interval: Option<IntervalProgress<'_>>,
) -> std::result::Result<(), PlayoutFailure> {
    let start_tokio = tokio::time::Instant::now();
    let start_std = Instant::now();
    if let Some(interval) = interval {
        if feedback.played() < interval.prior_target {
            let (played, at) = interval.clock.snapshot();
            let since = start_tokio.saturating_duration_since(at);
            let at_std = start_std.checked_sub(since).unwrap_or(start_std);
            watch.resume_progress(played, at_std);
        } else {
            interval.clock.observe(feedback.played(), true, start_tokio);
        }
    }
    loop {
        // Map Tokio's (possibly paused) clock onto the watch's std clock so
        // tests can drive stalls and watchdogs deterministically.
        let now_tokio = tokio::time::Instant::now();
        let now = start_std + now_tokio.saturating_duration_since(start_tokio);
        let played = feedback.played();
        let state = watch.poll(played, feedback.is_failed(), now);
        if let Some(interval) = interval {
            interval
                .clock
                .observe(played, state == PlayoutState::Played, now_tokio);
        }
        match state {
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
        pub finish_calls: usize,
        /// Like `CpalSink`, a drop request is served by the next callback.
        flush_requested: bool,
        flush_served: Option<u64>,
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
                finish_calls: 0,
                flush_requested: false,
                flush_served: None,
            }
        }

        /// Simulated device callback consuming up to `n` real samples,
        /// after serving any pending drop request.
        pub fn callback(&mut self, n: usize) -> usize {
            if std::mem::take(&mut self.flush_requested) {
                self.flush_served = Some(self.consumer.drain() as u64);
            }
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
        fn finish_input(&mut self) -> Result<()> {
            self.finish_calls += 1;
            Ok(())
        }
        fn discard_buffered(&mut self) -> Result<Option<u64>> {
            if let Some(dropped) = self.flush_served.take() {
                return Ok(Some(dropped));
            }
            self.flush_requested = true;
            Ok(None)
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
    fn worker_finishes_each_non_aligned_transmission_only_after_its_last_write() {
        let (mut prod, mut cons) = audio_ring(4096);
        let feedback = Arc::new(OutputFeedback::default());
        let mut worker = OutputWorker::new(1024, 48_000, feedback.clone());
        let mut sink = SimSink::new(4096);
        sink.write_limits = vec![Some(100), Some(0), Some(333), None];
        let frame = ramp(1503);
        prod.write(&frame);
        feedback.finish_through(frame.len() as u64);
        while sink.finish_calls == 0 {
            worker.step(&mut cons, &mut sink, sim_progress).unwrap();
        }
        assert_eq!(sink.finish_calls, 1);
        assert_eq!(sink.callback(4096), frame.len());
        worker.step(&mut cons, &mut sink, sim_progress).unwrap();
        assert_eq!(feedback.played(), frame.len() as u64);
        assert_eq!(sink.captured, frame);
    }

    #[test]
    fn worker_does_not_forward_next_frame_across_a_finish_boundary() {
        let (mut producer, mut consumer) = audio_ring(128);
        let feedback = Arc::new(OutputFeedback::default());
        let mut worker = OutputWorker::new(128, 48_000, feedback.clone());
        let mut sink = SimSink::new(128);
        let mut both = vec![1.0; 37];
        both.extend(vec![2.0; 43]);
        assert_eq!(producer.write(&both), both.len());
        feedback.finish_through(37);
        feedback.finish_through(80);
        assert_eq!(
            worker.step(&mut consumer, &mut sink, sim_progress).unwrap(),
            37
        );
        assert_eq!(sink.finish_calls, 1);
        assert_eq!(sink.callback(128), 37);
        assert_eq!(sink.captured, vec![1.0; 37]);
        assert_eq!(
            worker.step(&mut consumer, &mut sink, sim_progress).unwrap(),
            43
        );
        assert_eq!(sink.finish_calls, 2);
        assert_eq!(sink.callback(128), 43);
        assert_eq!(&sink.captured[37..], &vec![2.0; 43]);
    }

    #[test]
    fn discard_clears_unfinished_tail_before_next_transmission() {
        let (mut prod, mut cons) = audio_ring(4096);
        let feedback = Arc::new(OutputFeedback::default());
        let mut worker = OutputWorker::new(1024, 48_000, feedback.clone());
        let mut sink = SimSink::new(1024);
        sink.write_limits = vec![Some(0), Some(0), Some(128)];
        prod.write(&vec![1.0; 1000]);
        feedback.finish_through(1000);
        worker.step(&mut cons, &mut sink, sim_progress).unwrap();
        feedback.discard_through(1000);
        sink.callback(0);
        worker.step(&mut cons, &mut sink, sim_progress).unwrap();
        assert_eq!(sink.finish_calls, 0);
        prod.write(&vec![2.0; 100]);
        feedback.finish_through(1100);
        for _ in 0..20 {
            worker.step(&mut cons, &mut sink, sim_progress).unwrap();
            sink.callback(1024);
        }
        assert_eq!(sink.finish_calls, 1);
        assert_eq!(sink.captured, vec![2.0; 100]);
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

    /// A cancel discards the outer ring's contents, the retained chunk and
    /// the sink's own buffered tail, and later audio still completes against
    /// its cumulative target: none of the cancelled audio plays ahead of it.
    #[test]
    fn discard_drops_unplayed_samples_and_keeps_targets_aligned() {
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
        assert_eq!(sink.callback(0), 0);
        // Cancel: the owner drops its FIFO and asks the worker to discard.
        pending.clear();
        let through = pending.submitted();
        feedback.discard_through(through);
        let next: Vec<f32> = (0..5_000).map(|i| i as f32 + 2.0).collect();
        let target = pending.push(&next);
        let mut settled_before_next = false;
        while feedback.played() < target {
            pending.flush(&mut prod);
            worker.step(&mut cons, &mut sink, sim_progress).unwrap();
            if sink.captured.is_empty() && discard_settled(&feedback, through) {
                settled_before_next = true;
            }
            sink.callback(700);
        }
        assert!(
            settled_before_next,
            "the discard settles before new audio plays"
        );
        assert_eq!(sink.captured, next, "no cancelled sample may play");
        assert_eq!(feedback.played(), target);
    }

    /// Without a live consumer (none wired, or its end dropped), nothing is
    /// queued, so undeliverable TX audio cannot accumulate.
    #[test]
    fn tx_output_queues_nothing_without_a_live_ring() {
        let mut out = TxOutput::default();
        assert_eq!(out.submit(&[0.5; 10_000]), None);
        let (prod, cons) = audio_ring(1024);
        out.set_producer(prod);
        assert_eq!(out.submit(&[0.5; 3_000]), Some(3_000));
        assert_eq!(out.pending_len(), 3_000 - 1024);
        drop(cons);
        assert_eq!(out.submit(&[0.5; 10_000]), None);
        assert_eq!(out.pending_len(), 3_000 - 1024);
        assert_eq!(out.cancel(), 3_000 - 1024);
        assert_eq!(out.submitted(), 1024);
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
            wait_for_playout(&fb, watch, None).await,
            Err(PlayoutFailure::Stalled)
        );
        assert!(started.elapsed() >= STALL_TIMEOUT);
    }
}
