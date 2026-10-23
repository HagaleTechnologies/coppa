//! TNC (Terminal Node Controller) daemon mode.
//! Wires AFSK 1200 modem <-> CPAL audio <-> KISS TCP server <-> PTT control.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;

use coppa_audio::AudioRingProducer;
use coppa_radio::{NullPtt, PttControl, PttState, VoxPtt};

use crate::audio_output::{
    discard_settled, OutputFeedback, PendingOutput, PlayoutState, PlayoutWatch,
    DISCARD_SETTLE_TIMEOUT, PROGRESS_TICK,
};

/// PTT tail held after a TNC frame's last sample has played.
const TNC_TAIL: Duration = Duration::from_millis(50);

/// Modulated frames held behind the active one. `run_tnc` stops receiving
/// KISS frames while this backlog is full, so a client sending faster than
/// 1200 baud is backpressured by the KISS server's bounded channel instead
/// of growing this queue without limit.
const MAX_QUEUED_FRAMES: usize = 1;

/// Where the active TNC transmission is.
#[derive(Debug)]
enum TncPhase {
    /// Waiting for the frame's last sample to play.
    Playing(PlayoutWatch),
    /// No playback feedback (no output device): nominal duration.
    Nominal(Instant),
    /// Last sample played; holding PTT for the tail.
    Tail(Instant),
    /// Cancelled with PTT released: holding the next frame until every
    /// cancelled sample below `through` is dropped (including the final
    /// sink's buffered tail) or `until` passes, so the stale tail cannot
    /// play at the start of the next keyed frame.
    Discarding { through: u64, until: Instant },
}

/// TNC transmit state: one keyed frame at a time, later frames queued
/// behind it (`run_tnc` queues at most `MAX_QUEUED_FRAMES`, by receiving
/// only while [`has_room`](Self::has_room)), every sample retained
/// until the audio-out ring accepts it, and PTT released only after the
/// frame's last sample is predicted to have played plus the 50 ms tail.
/// Advanced from `run_tnc`'s select loop so receive forwarding and shutdown
/// stay serviceable while TX is backpressured.
pub struct TncTx {
    frames: VecDeque<Vec<f32>>,
    pending: PendingOutput,
    active: Option<TncPhase>,
    feedback: Option<Arc<OutputFeedback>>,
    sample_rate: u32,
}

impl TncTx {
    /// `feedback` is the output worker's playback progress; `None` when no
    /// output device is wired.
    pub fn new(feedback: Option<Arc<OutputFeedback>>, sample_rate: u32) -> Self {
        Self {
            frames: VecDeque::new(),
            pending: PendingOutput::default(),
            active: None,
            feedback,
            sample_rate: sample_rate.max(1),
        }
    }

    /// Queue a modulated frame for transmission. Callers check
    /// [`has_room`](Self::has_room) first; `submit` itself does not drop.
    pub fn submit(&mut self, samples: Vec<f32>) {
        if !samples.is_empty() {
            self.frames.push_back(samples);
        }
    }

    /// True while another frame may be queued behind the active one.
    pub fn has_room(&self) -> bool {
        self.frames.len() < MAX_QUEUED_FRAMES
    }

    /// True while a frame is keyed or queued.
    pub fn is_busy(&self) -> bool {
        self.active.is_some() || !self.frames.is_empty()
    }

    /// Advance TX: move retained samples into `out`, start the next frame,
    /// and complete or cancel the active one.
    pub fn poll(&mut self, now: Instant, out: &mut AudioRingProducer, ptt: &mut dyn PttControl) {
        self.pending.flush(out);
        match self.active.as_mut() {
            None => {
                let Some(frame) = self.frames.pop_front() else {
                    return;
                };
                if let Err(e) = ptt.set_ptt(PttState::Tx) {
                    eprintln!("PTT error: {}", e);
                }
                self.active = Some(match self.feedback {
                    Some(_) => {
                        let target = self.pending.push(&frame);
                        self.pending.flush(out);
                        TncPhase::Playing(PlayoutWatch::new(target, now, None))
                    }
                    None => {
                        // Nothing drains the ring without an output device,
                        // so don't queue samples that could never play.
                        let ms = frame.len() as u64 * 1000 / self.sample_rate as u64;
                        TncPhase::Nominal(now + Duration::from_millis(ms))
                    }
                });
            }
            Some(TncPhase::Playing(watch)) => {
                let (played, failed) = match self.feedback.as_ref() {
                    Some(fb) => (fb.played(), fb.is_failed()),
                    None => (0, true),
                };
                match watch.poll(played, failed, now) {
                    PlayoutState::Pending => {}
                    PlayoutState::Played => self.active = Some(TncPhase::Tail(now + TNC_TAIL)),
                    PlayoutState::Failed(why) => {
                        let discarded = self.pending.clear();
                        let through = self.pending.submitted();
                        if let Some(fb) = self.feedback.as_ref() {
                            fb.discard_through(through);
                        }
                        eprintln!(
                            "TNC transmission cancelled ({}); {} samples discarded",
                            why, discarded
                        );
                        let _ = ptt.set_ptt(PttState::Rx);
                        let settled = self
                            .feedback
                            .as_ref()
                            .is_none_or(|fb| discard_settled(fb, through));
                        self.active = (!settled).then(|| TncPhase::Discarding {
                            through,
                            until: now + DISCARD_SETTLE_TIMEOUT,
                        });
                    }
                }
            }
            Some(TncPhase::Nominal(until)) => {
                if now >= *until {
                    self.active = Some(TncPhase::Tail(now + TNC_TAIL));
                }
            }
            Some(TncPhase::Tail(until)) => {
                if now >= *until {
                    let _ = ptt.set_ptt(PttState::Rx);
                    self.active = None;
                }
            }
            Some(TncPhase::Discarding { through, until }) => {
                let settled = self
                    .feedback
                    .as_ref()
                    .is_none_or(|fb| discard_settled(fb, *through));
                if !settled && now >= *until {
                    // The output worker still holds new audio back until
                    // the sink confirms the drop.
                    eprintln!("TNC: cancelled audio not yet dropped by the output");
                }
                if settled || now >= *until {
                    self.active = None;
                }
            }
        }
    }
}

/// Configuration for TNC daemon mode.
pub struct TncConfig {
    pub kiss_port: u16,
    /// Address the KISS TCP server binds to. Defaults to "127.0.0.1" (loopback only).
    pub bind_address: String,
    pub rig_address: Option<String>,
    pub vox_mode: bool,
    pub sample_rate: u32,
    /// Audio device name substring (case-insensitive) for both input and
    /// output; `None` uses the default devices.
    pub audio_device: Option<String>,
}

impl Default for TncConfig {
    fn default() -> Self {
        Self {
            kiss_port: 8001,
            bind_address: "127.0.0.1".to_string(),
            rig_address: None,
            vox_mode: false,
            sample_rate: 48000,
            audio_device: None,
        }
    }
}

/// Run the TNC daemon.
///
/// Wires together:
/// - KISS TCP server (accepts AX.25 frames from host apps)
/// - AFSK 1200 demodulator (decodes audio → AX.25 frames)
/// - AFSK 1200 modulator (encodes AX.25 frames → audio)
/// - PTT control (keys the radio during TX)
/// - CPAL audio I/O (optional, enabled with `cpal-backend` feature)
pub async fn run_tnc(config: TncConfig) -> Result<()> {
    println!(
        "TNC mode: KISS port {}, sample rate {} Hz",
        config.kiss_port, config.sample_rate
    );

    // -------------------------------------------------------------------------
    // 1. KISS server
    // -------------------------------------------------------------------------
    let mut kiss_server =
        coppa_host::kiss::KissServer::with_bind_addr(config.kiss_port, config.bind_address.clone());
    let tx_receiver = kiss_server
        .take_tx_receiver()
        .expect("take_tx_receiver called twice");
    let rx_sender = kiss_server.rx_sender();

    // Spawn the KISS TCP accept loop
    tokio::spawn(async move {
        if let Err(e) = kiss_server.start().await {
            eprintln!("KISS server error: {}", e);
        }
    });

    println!("KISS server listening on port {}", config.kiss_port);

    // -------------------------------------------------------------------------
    // 2. PTT
    // -------------------------------------------------------------------------
    let mut ptt: Box<dyn PttControl> = if config.vox_mode {
        println!("PTT: VOX mode");
        Box::new(VoxPtt::new())
    } else if let Some(ref addr) = config.rig_address {
        println!("PTT: rigctld at {}", addr);
        match coppa_radio::rigctld::RigctldClient::connect(addr) {
            Ok(client) => Box::new(client),
            Err(e) => {
                eprintln!(
                    "Failed to connect to rigctld ({}), falling back to NullPtt",
                    e
                );
                Box::new(NullPtt::new())
            }
        }
    } else {
        println!("PTT: null (no PTT configured)");
        Box::new(NullPtt::new())
    };

    // -------------------------------------------------------------------------
    // 3. Audio ring buffers
    // -------------------------------------------------------------------------
    let (audio_in_prod, audio_in_cons) = coppa_audio::audio_ring(65536);
    let (mut audio_out_prod, audio_out_cons) = coppa_audio::audio_ring(65536);
    #[cfg(feature = "cpal-backend")]
    let output_feedback = Some(Arc::new(OutputFeedback::default()));
    #[cfg(not(feature = "cpal-backend"))]
    let output_feedback: Option<Arc<OutputFeedback>> = None;
    let mut tnc_tx = TncTx::new(output_feedback.clone(), config.sample_rate);

    // -------------------------------------------------------------------------
    // 4. Shutdown flag
    // -------------------------------------------------------------------------
    let shutdown = Arc::new(AtomicBool::new(false));

    // -------------------------------------------------------------------------
    // 5. Demod thread
    // -------------------------------------------------------------------------
    let (demod_tx, mut demod_rx) = mpsc::channel::<Vec<u8>>(64);
    let demod_shutdown = shutdown.clone();

    tokio::task::spawn_blocking(move || {
        let mut demod = coppa_codec::afsk::Demodulator::new();
        let mut audio_in = audio_in_cons;
        let mut buf = vec![0.0f32; 1024];

        loop {
            if demod_shutdown.load(Ordering::Acquire) || audio_in.is_abandoned() {
                break;
            }
            let n = audio_in.read(&mut buf);
            if n > 0 {
                demod.process(&buf[..n]);
                for frame in demod.take_frames() {
                    // Best-effort send; if the main loop is gone, stop.
                    if demod_tx.blocking_send(frame).is_err() {
                        return;
                    }
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    });

    // -------------------------------------------------------------------------
    // 6. CPAL audio I/O (optional)
    // -------------------------------------------------------------------------
    #[cfg(feature = "cpal-backend")]
    {
        use coppa_audio::{AudioSink, AudioSource};

        let sample_rate = config.sample_rate;
        let buf_size = 8192usize;
        let device_name = config.audio_device.clone();

        // Input: CPAL mic → audio_in ring buffer
        let mut mic_prod = audio_in_prod;
        let input_shutdown = shutdown.clone();
        let input_result = match device_name.as_deref() {
            Some(name) => match coppa_audio::find_input_device_by_name(name) {
                Some(device) => coppa_audio::CpalSource::from_device(device, sample_rate, buf_size),
                None => {
                    eprintln!(
                        "WARNING: No input device matching '{}', using default",
                        name
                    );
                    coppa_audio::CpalSource::new(sample_rate, buf_size)
                }
            },
            None => coppa_audio::CpalSource::new(sample_rate, buf_size),
        };
        match input_result {
            Ok(mut source) => {
                if let Err(e) = source.start() {
                    eprintln!("Failed to start audio input: {}", e);
                } else {
                    println!("Audio input started ({}Hz)", sample_rate);
                    tokio::task::spawn_blocking(move || {
                        let mut buf = vec![0.0f32; 1024];
                        loop {
                            if mic_prod.is_abandoned() || input_shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            match source.read(&mut buf) {
                                Ok(n) if n > 0 => {
                                    mic_prod.write(&buf[..n]);
                                }
                                _ => {
                                    std::thread::sleep(std::time::Duration::from_millis(10));
                                }
                            }
                        }
                    });
                }
            }
            Err(e) => eprintln!("No audio input device: {}", e),
        }

        // Output: audio_out ring buffer → CPAL speaker. The sink lives on
        // its worker thread, and the worker retains each chunk until the sink
        // accepts all of it, reporting playback progress back to `tnc_tx`.
        let output_shutdown = shutdown.clone();
        let feedback = output_feedback
            .clone()
            .expect("cpal-backend always wires output feedback");
        tokio::task::spawn_blocking(move || {
            let sink_result = match device_name.as_deref() {
                Some(name) => match coppa_audio::find_output_device_by_name(name) {
                    Some(device) => {
                        coppa_audio::CpalSink::from_device(device, sample_rate, buf_size)
                    }
                    None => {
                        eprintln!(
                            "WARNING: No output device matching '{}', using default",
                            name
                        );
                        coppa_audio::CpalSink::new(sample_rate, buf_size)
                    }
                },
                None => coppa_audio::CpalSink::new(sample_rate, buf_size),
            };
            let mut sink = match sink_result {
                Ok(sink) => sink,
                Err(e) => {
                    eprintln!("No audio output device: {}", e);
                    feedback.fail();
                    return;
                }
            };
            if let Err(e) = sink.start() {
                eprintln!("Failed to start audio output: {}", e);
                feedback.fail();
                return;
            }
            println!("Audio output started ({}Hz)", sample_rate);
            crate::audio_output::OutputWorker::new(1024, sample_rate, feedback).run(
                audio_out_cons,
                sink,
                |sink| sink.playback_progress(),
                output_shutdown,
            );
        });
    }

    // Suppress unused variable warnings when cpal-backend is not enabled.
    #[cfg(not(feature = "cpal-backend"))]
    {
        let _ = audio_in_prod;
        let _ = audio_out_cons;
    }

    // -------------------------------------------------------------------------
    // 7. Main event loop
    // -------------------------------------------------------------------------
    let mut tx_receiver = tx_receiver;
    let mut tx_tick = tokio::time::interval(PROGRESS_TICK);
    tx_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    println!("TNC ready.");

    loop {
        tokio::select! {
            // RX path: demod thread produced a frame → forward to KISS clients
            Some(frame) = demod_rx.recv() => {
                let _ = rx_sender.send(frame);
            }

            // TX path: KISS client sent a frame → modulate and queue it;
            // `tnc_tx` keys PTT, paces it out and unkeys after it played.
            // Not polled while the backlog is full, so later frames wait in
            // the KISS server's bounded channel.
            Some(ax25_data) = tx_receiver.recv(), if tnc_tx.has_room() => {
                tnc_tx.submit(coppa_codec::afsk::modulate(&ax25_data));
                tnc_tx.poll(Instant::now(), &mut audio_out_prod, ptt.as_mut());
            }

            _ = tx_tick.tick(), if tnc_tx.is_busy() => {
                tnc_tx.poll(Instant::now(), &mut audio_out_prod, ptt.as_mut());
            }

            // Ctrl-C: graceful shutdown
            _ = tokio::signal::ctrl_c() => {
                println!("\nTNC shutting down...");
                shutdown.store(true, Ordering::Release);
                if tnc_tx.is_busy() {
                    let _ = ptt.set_ptt(PttState::Rx);
                }
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_output::test_support::{sim_progress, SimSink};
    use crate::audio_output::{OutputWorker, STALL_TIMEOUT};
    use coppa_radio::PttState;
    use std::sync::Mutex;

    /// PTT controller recording every state change.
    #[derive(Default, Clone)]
    struct RecordingPtt(Arc<Mutex<Vec<PttState>>>);

    impl PttControl for RecordingPtt {
        fn set_ptt(&mut self, state: PttState) -> Result<()> {
            self.0.lock().unwrap().push(state);
            Ok(())
        }
        fn get_ptt(&mut self) -> Result<PttState> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .last()
                .copied()
                .unwrap_or(PttState::Rx))
        }
    }

    struct Harness {
        tx: TncTx,
        out: AudioRingProducer,
        outer: coppa_audio::AudioRingConsumer,
        worker: OutputWorker,
        sink: SimSink,
        feedback: Arc<OutputFeedback>,
        ptt: RecordingPtt,
        now: Instant,
    }

    impl Harness {
        fn new() -> Self {
            // Production capacities: 65,536-sample TNC ring, 8,192 sink ring.
            let (out, outer) = coppa_audio::audio_ring(65536);
            let feedback = Arc::new(OutputFeedback::default());
            Self {
                tx: TncTx::new(Some(feedback.clone()), 48_000),
                out,
                outer,
                worker: OutputWorker::new(1024, 48_000, feedback.clone()),
                sink: SimSink::new(8192),
                feedback,
                ptt: RecordingPtt::default(),
                now: Instant::now(),
            }
        }

        /// One 5 ms tick: TNC poll, worker forwarding, a device callback.
        fn tick(&mut self, callback: usize) {
            self.now += PROGRESS_TICK;
            self.tx.poll(self.now, &mut self.out, &mut self.ptt);
            for _ in 0..8 {
                self.worker
                    .step(&mut self.outer, &mut self.sink, sim_progress)
                    .unwrap();
            }
            self.sink.callback(callback);
        }

        fn run_until_idle(&mut self, callback: usize) {
            self.tx.poll(self.now, &mut self.out, &mut self.ptt);
            let mut ticks = 0;
            while self.tx.is_busy() {
                self.tick(callback);
                ticks += 1;
                assert!(ticks < 200_000, "TNC TX never completed");
            }
        }
    }

    fn frame(fill: u8) -> Vec<f32> {
        coppa_codec::afsk::modulate(&[fill; 256])
    }

    /// Real 256-byte AFSK frames (94,080 and 110,440 samples) exceed the
    /// 65,536-sample TNC ring; every sample must reach the final sink.
    #[test]
    fn afsk_frames_larger_than_the_ring_arrive_whole() {
        for fill in [0x00u8, 0xff] {
            let samples = frame(fill);
            assert!(samples.len() > 65_536);
            let mut h = Harness::new();
            // Delayed, partial downstream writes must not truncate either.
            h.sink.write_limits = vec![Some(300), Some(0), None, Some(7), Some(0)];
            h.tx.submit(samples.clone());
            h.run_until_idle(240);
            assert_eq!(h.sink.captured, samples, "fill {fill:#x}");
            assert_eq!(h.out.overflow_count(), 0);
            assert_eq!(*h.ptt.0.lock().unwrap(), vec![PttState::Tx, PttState::Rx]);
        }
    }

    /// The 50 ms tail starts after the last sample's predicted presentation,
    /// not when the rings empty.
    #[test]
    fn tail_follows_predicted_presentation() {
        let mut h = Harness::new();
        let samples = frame(0x00);
        h.sink.remaining = Duration::from_millis(40);
        h.tx.submit(samples.clone());
        h.tx.poll(h.now, &mut h.out, &mut h.ptt);
        while h.sink.captured.len() < samples.len() {
            h.tick(4096);
        }
        // All consumed, but 40 ms of presentation remains: still keyed.
        for _ in 0..20 {
            h.tick(0);
            assert_eq!(
                h.ptt.0.lock().unwrap().len(),
                1,
                "PTT released before playback ended"
            );
        }
        h.sink.remaining = Duration::ZERO;
        let played_at = h.now + PROGRESS_TICK;
        h.run_until_idle(0);
        assert!(h.now >= played_at + TNC_TAIL);
        assert_eq!(h.sink.captured, samples);
    }

    /// Two KISS frames play in order, each keyed and completed on its own.
    #[test]
    fn queued_frames_complete_in_order() {
        let mut h = Harness::new();
        let a = frame(0x00);
        let b = coppa_codec::afsk::modulate(b"second frame");
        h.tx.submit(a.clone());
        h.tx.submit(b.clone());
        h.run_until_idle(1024);
        let mut expected = a;
        expected.extend_from_slice(&b);
        assert_eq!(h.sink.captured, expected);
        assert_eq!(
            *h.ptt.0.lock().unwrap(),
            vec![PttState::Tx, PttState::Rx, PttState::Tx, PttState::Rx]
        );
    }

    /// `run_tnc`'s TX arm (submit, then poll) is only polled while
    /// `has_room`, so the TNC holds at most one modulated frame behind the
    /// active one; a faster KISS client waits in the server's bounded
    /// channel instead of growing the queue.
    #[test]
    fn backlog_behind_the_active_frame_is_bounded() {
        let mut h = Harness::new();
        let mut accepted = 0;
        for _ in 0..10 {
            if !h.tx.has_room() {
                break;
            }
            h.tx.submit(frame(0x00));
            h.tx.poll(h.now, &mut h.out, &mut h.ptt);
            accepted += 1;
        }
        assert_eq!(accepted, 1 + MAX_QUEUED_FRAMES);
        assert_eq!(h.tx.frames.len(), MAX_QUEUED_FRAMES);
        // Once the active frame has played, the queued one is promoted and
        // the TNC accepts again.
        let mut ticks = 0;
        while !h.tx.has_room() {
            h.tick(4096);
            ticks += 1;
            assert!(ticks < 200_000, "queued frame never promoted");
        }
        assert!(h.tx.frames.is_empty());
        assert!(h.tx.is_busy(), "the queued frame is now the active one");
    }

    /// A stalled output cancels the frame, releases PTT and discards the
    /// retained suffix, so it cannot play later.
    #[test]
    fn stalled_output_cancels_and_discards() {
        let mut h = Harness::new();
        let samples = frame(0xff);
        h.tx.submit(samples);
        h.tx.poll(h.now, &mut h.out, &mut h.ptt);
        let deadline = h.now + STALL_TIMEOUT + PROGRESS_TICK;
        while h.now < deadline {
            h.now += PROGRESS_TICK;
            h.tx.poll(h.now, &mut h.out, &mut h.ptt);
        }
        assert_eq!(*h.ptt.0.lock().unwrap(), vec![PttState::Tx, PttState::Rx]);
        // The output never confirms the drop (no worker runs here), so the
        // TNC stays unkeyed and idle for at most the settle bound.
        let settle_by = h.now + DISCARD_SETTLE_TIMEOUT;
        while h.tx.is_busy() {
            assert!(h.now <= settle_by, "discard hold exceeded its bound");
            h.now += PROGRESS_TICK;
            h.tx.poll(h.now, &mut h.out, &mut h.ptt);
        }
        assert_eq!(*h.ptt.0.lock().unwrap(), vec![PttState::Tx, PttState::Rx]);
        assert!(h.tx.pending.is_empty());
        assert_eq!(
            h.tx.pending.submitted(),
            65_536,
            "only ring-held samples remain counted"
        );
    }

    /// A frame cancelled with its tail already buffered in the final sink's
    /// ring must not play that tail at the start of the next frame: the sink
    /// is flushed before the next frame's audio reaches it.
    #[test]
    fn cancelled_tail_in_sink_does_not_open_next_frame() {
        let mut h = Harness::new();
        let next = coppa_codec::afsk::modulate(b"next frame");
        h.tx.submit(frame(0xff));
        h.tx.submit(next.clone());
        // The device consumes nothing until the stall fires, while the
        // worker fills the sink's 8,192-sample ring with the doomed frame.
        let deadline = h.now + STALL_TIMEOUT + PROGRESS_TICK;
        while h.now < deadline {
            h.tick(0);
        }
        assert!(h.sink.captured.is_empty());
        assert_eq!(*h.ptt.0.lock().unwrap(), vec![PttState::Tx, PttState::Rx]);
        h.run_until_idle(1024);
        assert_eq!(
            h.sink.captured, next,
            "the cancelled frame's buffered tail played ahead of the next frame"
        );
        assert_eq!(
            *h.ptt.0.lock().unwrap(),
            vec![PttState::Tx, PttState::Rx, PttState::Tx, PttState::Rx]
        );
    }

    /// Output failure cancels immediately.
    #[test]
    fn output_failure_cancels() {
        let mut h = Harness::new();
        h.tx.submit(frame(0x00));
        h.tx.poll(h.now, &mut h.out, &mut h.ptt);
        h.feedback.fail();
        h.tick(0);
        assert!(!h.tx.is_busy());
        assert_eq!(*h.ptt.0.lock().unwrap(), vec![PttState::Tx, PttState::Rx]);
    }
}
