//! COP-169 floor-mode measurement contract: the SNR grid, locked seed sets,
//! conservative receive-stage accounting, the 50% FER crossing and the
//! acceptance verdict, plus a sweep driver that runs the PRODUCTION receiver
//! through the persistent channel ([`coppa_channel::persistent`]).
//!
//! These rules are fixed before any floor waveform is tuned, so a candidate
//! cannot pass by changing the metric. The acceptance bars come from the
//! ticket: the 50% crossing at or below −8 dB on AWGN and at or below −2 dB on
//! Watterson Poor (1 Hz / 2 ms), and `SyncFailed` under half of the failures at
//! every grid point above the crossing.
//!
//! # Accounting
//!
//! Every attempt lands in exactly one [`AttemptOutcome`]. `SyncFailed` keeps its
//! conservative meaning: no trustworthy header, whether nothing was detected,
//! the detection was mistimed, or the protected header failed to decode. Those
//! causes get diagnostic subcounts, but all three stay in the `sync_failed`
//! numerator the acceptance rule uses.
//!
//! # Crossing
//!
//! The crossing is found by linear interpolation between adjacent grid points
//! whose FER brackets 0.5 (a point at exactly 0.5 counts as "at or above").
//! A non-monotone curve reports its highest crossing and lists every
//! recrossing. A curve below 0.5 everywhere reports an upper bound at the grid
//! minimum. A curve that is still at or above 0.5 at the top of the grid has no
//! passing crossing.

use coppa_channel::persistent::{SnrProfile, VirtualCable};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::streaming::StreamingReceiver;
use coppa_protocol::modem::transceiver::{CoppaTransceiver, ReceiveError};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::metrics::wilson_ci95;
use crate::scenario::SAMPLE_RATE;

/// Development seed base: tune only on `DEV_SEED_BASE + i`.
pub const DEV_SEED_BASE: u64 = 0x00C0_FFEE;
/// Final sweep seed base: acceptance sweeps use `FINAL_SWEEP_SEED_BASE + i`.
pub const FINAL_SWEEP_SEED_BASE: u64 = 0x1690_0000;
/// Final session seed base: connect and ramp trials use `FINAL_SESSION_SEED_BASE + i`.
pub const FINAL_SESSION_SEED_BASE: u64 = 0x1691_0000;

/// Lowest grid SNR in dB (3 kHz reference).
pub const GRID_MIN_DB: i32 = -12;
/// Highest grid SNR in dB.
pub const GRID_MAX_DB: i32 = 4;
/// Grid step in dB.
pub const GRID_STEP_DB: i32 = 2;
/// Trials per grid point for the acceptance sweep.
pub const TRIALS_PER_POINT: usize = 200;

/// Acceptance bar for the AWGN 50% crossing, dB.
pub const AWGN_CROSSING_BAR_DB: f64 = -8.0;
/// Acceptance bar for the Watterson Poor 50% crossing, dB.
pub const POOR_CROSSING_BAR_DB: f64 = -2.0;

/// Seconds of channel noise before each frame, so the receiver searches for
/// the frame instead of being handed its start.
pub const LEAD_IN_S: f64 = 0.35;
/// Seconds of channel noise after each frame (covers the channel latency).
pub const TAIL_S: f64 = 0.25;

/// Chunk size the streaming receive path is fed through: 100 ms at 48 kHz.
/// Continuous acquisition is exercised by pushing the whole capture
/// (lead-in, frame, tail) through in these fixed arbitrary-size chunks.
pub const STREAM_CHUNK_SAMPLES: usize = 4_800;

/// Which production receive path an attempt is measured through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxPath {
    /// Whole-capture [`CoppaTransceiver::receive`], no oracle timing.
    Batch,
    /// [`StreamingReceiver::push_samples`] over continuous fixed-size chunks.
    Streaming,
}

impl RxPath {
    /// CSV name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Batch => "batch",
            Self::Streaming => "streaming",
        }
    }
}

/// The CSV header for one sweep row.
pub const CSV_HEADER: &str = "waveform,rx_path,channel,snr_db,trials,frame_errors,sync_failed,\
sync_no_detection,sync_mistimed,sync_header_decode,header_corrupt,payload_failed,wrong_payload,\
fer,fer_lo,fer_hi,airtime_s,goodput_bps";

/// The inclusive SNR grid from `min_db` to `max_db` in `step_db` steps.
/// Returns an empty grid for a non-positive step or `min_db > max_db`.
#[must_use]
pub fn snr_grid(min_db: i32, max_db: i32, step_db: i32) -> Vec<i32> {
    if step_db <= 0 || min_db > max_db {
        return Vec::new();
    }
    (min_db..=max_db).step_by(step_db as usize).collect()
}

/// The ticket's acceptance grid: −12 to +4 dB in 2 dB steps (nine points).
#[must_use]
pub fn acceptance_grid() -> Vec<i32> {
    snr_grid(GRID_MIN_DB, GRID_MAX_DB, GRID_STEP_DB)
}

/// Seed of attempt `trial` at grid point `point`: `base + i` with
/// `i = point * trials_per_point + trial`, so every attempt in a sweep has its
/// own seed and no seed is reused across points. The channel derives its
/// direction, fading and noise streams from this seed
/// ([`coppa_channel::persistent::derive_seed`]).
#[must_use]
pub fn attempt_seed(base: u64, point: usize, trials_per_point: usize, trial: usize) -> u64 {
    base.wrapping_add((point * trials_per_point + trial) as u64)
}

/// Why no trustworthy header was found. All three count as `SyncFailed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncCause {
    /// Nothing was detected.
    NoDetection,
    /// A detection was made at the wrong place.
    Mistimed,
    /// A detection was made but the protected header did not decode.
    HeaderDecode,
}

/// The single outcome of one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The exact payload was delivered.
    Delivered,
    /// No trustworthy header (conservative `SyncFailed`).
    SyncFailed(SyncCause),
    /// A header decoded and passed its check but its fields were invalid.
    /// Only a receiver that can tell this from a failed header decode reports
    /// it; anything less certain is [`SyncCause::HeaderDecode`].
    HeaderCorrupt,
    /// The header was trusted but the payload did not decode.
    PayloadFailed,
    /// A payload was delivered that differs from the one sent.
    WrongPayload,
}

impl AttemptOutcome {
    /// Classify a batch [`CoppaTransceiver::receive`] result. The legacy
    /// receiver does not say which acquisition stage failed, so its
    /// `SyncFailed` is recorded as [`SyncCause::NoDetection`]. Its
    /// `HeaderCorrupt` covers both a protected header that did not decode and
    /// one with impossible fields, and it cannot tell the two apart, so all of
    /// it is counted conservatively as [`SyncCause::HeaderDecode`].
    #[must_use]
    pub fn from_receive(result: Result<&[u8], &ReceiveError>, sent: &[u8]) -> Self {
        match result {
            Ok(rx) if rx == sent => Self::Delivered,
            Ok(_) => Self::WrongPayload,
            Err(ReceiveError::SyncFailed) => Self::SyncFailed(SyncCause::NoDetection),
            Err(ReceiveError::HeaderCorrupt) => Self::SyncFailed(SyncCause::HeaderDecode),
            Err(ReceiveError::LdpcNotConverged { .. } | ReceiveError::CrcMismatch) => {
                Self::PayloadFailed
            }
        }
    }
}

/// Tally of one grid point.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PointTally {
    /// SNR in dB.
    pub snr_db: f64,
    /// Attempts made.
    pub trials: usize,
    /// Attempts that delivered the exact payload.
    pub delivered: usize,
    /// Conservative `SyncFailed` (sum of the three causes below).
    pub sync_failed: usize,
    /// `SyncFailed` because nothing was detected.
    pub sync_no_detection: usize,
    /// `SyncFailed` because the detection was mistimed.
    pub sync_mistimed: usize,
    /// `SyncFailed` because the protected header did not decode.
    pub sync_header_decode: usize,
    /// Header decoded with invalid fields.
    pub header_corrupt: usize,
    /// Payload decode failed.
    pub payload_failed: usize,
    /// Wrong payload delivered.
    pub wrong_payload: usize,
    /// Total waveform airtime of all attempts, in samples.
    pub airtime_samples: u64,
    /// Application bytes delivered correctly.
    pub delivered_bytes: u64,
}

impl PointTally {
    /// An empty tally at `snr_db`.
    #[must_use]
    pub fn new(snr_db: f64) -> Self {
        Self {
            snr_db,
            ..Self::default()
        }
    }

    /// Record one attempt with its waveform airtime and payload size.
    pub fn record(
        &mut self,
        outcome: AttemptOutcome,
        airtime_samples: usize,
        payload_bytes: usize,
    ) {
        self.trials += 1;
        self.airtime_samples += airtime_samples as u64;
        match outcome {
            AttemptOutcome::Delivered => {
                self.delivered += 1;
                self.delivered_bytes += payload_bytes as u64;
            }
            AttemptOutcome::SyncFailed(cause) => {
                self.sync_failed += 1;
                match cause {
                    SyncCause::NoDetection => self.sync_no_detection += 1,
                    SyncCause::Mistimed => self.sync_mistimed += 1,
                    SyncCause::HeaderDecode => self.sync_header_decode += 1,
                }
            }
            AttemptOutcome::HeaderCorrupt => self.header_corrupt += 1,
            AttemptOutcome::PayloadFailed => self.payload_failed += 1,
            AttemptOutcome::WrongPayload => self.wrong_payload += 1,
        }
    }

    /// Every attempt that did not deliver the exact payload.
    #[must_use]
    pub fn frame_errors(&self) -> usize {
        self.sync_failed + self.header_corrupt + self.payload_failed + self.wrong_payload
    }

    /// True when the point has attempts and each is counted exactly once. A
    /// point with no attempts measured nothing, so it never passes.
    #[must_use]
    pub fn is_partition(&self) -> bool {
        self.trials > 0
            && self.delivered + self.frame_errors() == self.trials
            && self.sync_no_detection + self.sync_mistimed + self.sync_header_decode
                == self.sync_failed
    }

    /// Empirical FER (frame errors / trials); 0 with no trials.
    #[must_use]
    pub fn fer(&self) -> f64 {
        if self.trials == 0 {
            0.0
        } else {
            self.frame_errors() as f64 / self.trials as f64
        }
    }

    /// The sync-share clause: `2 * sync_failed < frame_errors`, and zero
    /// failures satisfies it (no division).
    #[must_use]
    pub fn sync_share_ok(&self) -> bool {
        self.frame_errors() == 0 || 2 * self.sync_failed < self.frame_errors()
    }

    /// Total airtime in seconds.
    #[must_use]
    pub fn airtime_s(&self) -> f64 {
        self.airtime_samples as f64 / SAMPLE_RATE as f64
    }

    /// Delivered application bits per second of waveform airtime.
    #[must_use]
    pub fn goodput_bps(&self) -> f64 {
        let t = self.airtime_s();
        if t > 0.0 {
            self.delivered_bytes as f64 * 8.0 / t
        } else {
            0.0
        }
    }

    /// One CSV row in [`CSV_HEADER`] order.
    #[must_use]
    pub fn csv_row(&self, waveform: &str, rx_path: &str, channel: &str) -> String {
        let (lo, hi) = wilson_ci95(self.frame_errors(), self.trials);
        format!(
            "{waveform},{rx_path},{channel},{},{},{},{},{},{},{},{},{},{},{:.4},{:.4},{:.4},{:.3},{:.2}",
            self.snr_db,
            self.trials,
            self.frame_errors(),
            self.sync_failed,
            self.sync_no_detection,
            self.sync_mistimed,
            self.sync_header_decode,
            self.header_corrupt,
            self.payload_failed,
            self.wrong_payload,
            self.fer(),
            lo,
            hi,
            self.airtime_s(),
            self.goodput_bps(),
        )
    }
}

/// Where a FER curve crosses 50%.
#[derive(Debug, Clone, PartialEq)]
pub enum Crossing {
    /// The highest interpolated crossing, with every crossing found (ascending).
    At {
        /// The highest crossing, dB.
        db: f64,
        /// Every crossing, ascending; more than one means the curve recrosses.
        all: Vec<f64>,
    },
    /// FER is below 0.5 at every grid point: the crossing is at or below this
    /// bound (the grid minimum).
    BelowGrid {
        /// The grid minimum, dB.
        upper_bound_db: f64,
    },
    /// FER is still at or above 0.5 at the top of the grid: no passing crossing.
    NoPassingCrossing {
        /// Any crossings found lower down, ascending.
        all: Vec<f64>,
    },
}

impl Crossing {
    /// The crossing value compared against a bar: the highest crossing, or the
    /// grid minimum when FER is below 0.5 everywhere; `None` when there is no
    /// passing crossing.
    #[must_use]
    pub fn db(&self) -> Option<f64> {
        match self {
            Self::At { db, .. } => Some(*db),
            Self::BelowGrid { upper_bound_db } => Some(*upper_bound_db),
            Self::NoPassingCrossing { .. } => None,
        }
    }
}

/// Find the 50% crossing of `(snr_db, fer)` points sorted by ascending SNR.
/// Returns `None` for an empty curve.
#[must_use]
pub fn fifty_percent_crossing(curve: &[(f64, f64)]) -> Option<Crossing> {
    let first = curve.first()?;
    let last = curve.last()?;
    let above = |fer: f64| fer >= 0.5;
    let mut all = Vec::new();
    for w in curve.windows(2) {
        let ((xa, fa), (xb, fb)) = (w[0], w[1]);
        if above(fa) != above(fb) {
            all.push(xa + (0.5 - fa) * (xb - xa) / (fb - fa));
        }
    }
    if above(last.1) {
        return Some(Crossing::NoPassingCrossing { all });
    }
    if all.is_empty() {
        return Some(Crossing::BelowGrid {
            upper_bound_db: first.0,
        });
    }
    let db = all.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(Crossing::At { db, all })
}

/// The acceptance verdict for one sweep.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// Where FER crosses 50% (`None` for an empty sweep).
    pub crossing: Option<Crossing>,
    /// The bar the crossing must be at or below, dB.
    pub bar_db: f64,
    /// Grid points above the crossing where `SyncFailed` is half of the
    /// failures or more.
    pub sync_share_violations: Vec<f64>,
    /// Wrong payloads delivered anywhere in the sweep (must be zero).
    pub wrong_payloads: usize,
    /// Grid points whose tallies do not partition their attempts.
    pub accounting_errors: Vec<f64>,
}

impl Verdict {
    /// True when the crossing meets the bar, the sync-share clause holds above
    /// it, no wrong payload was delivered and the accounting is exact (every
    /// point has attempts). An empty sweep has no crossing, so it fails.
    #[must_use]
    pub fn passes(&self) -> bool {
        let crossing_ok = self
            .crossing
            .as_ref()
            .and_then(Crossing::db)
            .is_some_and(|db| db <= self.bar_db);
        crossing_ok
            && self.sync_share_violations.is_empty()
            && self.wrong_payloads == 0
            && self.accounting_errors.is_empty()
    }
}

/// Evaluate a sweep (points in ascending SNR) against a crossing bar.
#[must_use]
pub fn evaluate(points: &[PointTally], bar_db: f64) -> Verdict {
    let curve: Vec<(f64, f64)> = points.iter().map(|p| (p.snr_db, p.fer())).collect();
    let crossing = fifty_percent_crossing(&curve);
    // "Above the crossing": every point when FER is below 0.5 everywhere; no
    // point when there is no passing crossing (the sweep already fails).
    let floor_db = match &crossing {
        Some(Crossing::At { db, .. }) => Some(*db),
        Some(Crossing::BelowGrid { .. }) => Some(f64::NEG_INFINITY),
        Some(Crossing::NoPassingCrossing { .. }) | None => None,
    };
    let sync_share_violations = floor_db.map_or_else(Vec::new, |floor| {
        points
            .iter()
            .filter(|p| p.snr_db > floor && !p.sync_share_ok())
            .map(|p| p.snr_db)
            .collect()
    });
    Verdict {
        crossing,
        bar_db,
        sync_share_violations,
        wrong_payloads: points.iter().map(|p| p.wrong_payload).sum(),
        accounting_errors: points
            .iter()
            .filter(|p| !p.is_partition())
            .map(|p| p.snr_db)
            .collect(),
    }
}

/// Channel of a floor sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepChannel {
    /// AWGN only.
    Awgn,
    /// Watterson Poor (1 Hz spread, 2 ms delay).
    Poor,
}

impl SweepChannel {
    /// CSV name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Awgn => "awgn",
            Self::Poor => "poor",
        }
    }

    /// The ticket's crossing bar for this channel.
    #[must_use]
    pub fn bar_db(self) -> f64 {
        match self {
            Self::Awgn => AWGN_CROSSING_BAR_DB,
            Self::Poor => POOR_CROSSING_BAR_DB,
        }
    }
}

/// Run one receive attempt of `tx` through a fresh persistent channel: noise
/// lead-in, the frame, a noise tail, then the selected production receive path
/// over the whole capture (no oracle timing or header). Returns the outcome and
/// the frame's airtime in samples.
///
/// # Errors
///
/// A payload over 65535 bytes or one `tx` refuses to transmit is a harness
/// configuration error, returned rather than counted as a channel failure.
#[allow(clippy::too_many_arguments)]
pub fn run_attempt(
    tx: &CoppaTransceiver,
    level: u8,
    payload_bytes: usize,
    channel: SweepChannel,
    rx_path: RxPath,
    snr_db: f32,
    seed: u64,
) -> anyhow::Result<(AttemptOutcome, usize)> {
    let payload_len = u16::try_from(payload_bytes).map_err(|_| {
        anyhow::anyhow!("payload of {payload_bytes} bytes exceeds the header's u16")
    })?;
    let mut rng = StdRng::seed_from_u64(seed);
    let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
    let header = CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: tx.profile().bandwidth_id,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len,
        codewords: 1,
    };
    let clean = tx
        .transmit(&header, &payload)
        .map_err(|e| anyhow::anyhow!("level {level} cannot carry {payload_bytes} bytes: {e:?}"))?;
    let p_clean = coppa_channel::mean_power(&clean);
    let fading = match channel {
        SweepChannel::Awgn => None,
        SweepChannel::Poor => Some(WattersonPreset::Poor.config()),
    };
    let mut cable = VirtualCable::new(
        SAMPLE_RATE as f32,
        fading.as_ref(),
        Some(SnrProfile::constant(snr_db)),
        p_clean,
        seed,
    );
    // Unknown alignment: the lead-in length varies per attempt.
    let jitter_s = f64::from(rng.random::<u16>()) / f64::from(u16::MAX) * 0.05;
    let mut capture = cable.idle(LEAD_IN_S + jitter_s).at_b;
    capture.extend(
        cable
            .transmit(coppa_channel::persistent::Direction::AToB, &clean)
            .at_b,
    );
    capture.extend(cable.idle(TAIL_S).at_b);
    // Like the legacy sweep, a decoded payload counts as delivered when its
    // first `payload.len()` bytes are the ones sent; a short one is wrong.
    let outcome = match rx_path {
        RxPath::Batch => {
            let result = tx.receive(&capture);
            tx.harq_evict(header.seq_num);
            match &result {
                Ok((_, got, _)) => {
                    AttemptOutcome::from_receive(Ok(&got[..got.len().min(payload.len())]), &payload)
                }
                Err(e) => AttemptOutcome::from_receive(Err(e), &payload),
            }
        }
        RxPath::Streaming => {
            // A fresh receiver per attempt: the streaming path is measured on
            // its own, carrying no state across attempts.
            let mut stream = StreamingReceiver::new(tx.profile().clone(), 1);
            let mut decoded = Vec::new();
            for chunk in capture.chunks(STREAM_CHUNK_SAMPLES) {
                decoded.extend(stream.push_samples(chunk));
            }
            let delivered_payloads: Vec<&[u8]> = decoded
                .iter()
                .map(|f| &f.payload[..f.payload.len().min(payload.len())])
                .collect();
            classify_streaming(
                &delivered_payloads,
                stream.header_only_decodes() > 0,
                &payload,
            )
        }
    };
    Ok((outcome, clean.len()))
}

/// Classify a streaming attempt from every payload its receiver delivered.
/// The streaming path emits only fully CRC-validated frames, so a delivered
/// frame that differs from the sent one is a wrong payload; conservatively,
/// one wrong delivery marks the attempt wrong even when the correct frame
/// also decoded. The path reports no per-candidate errors, but its
/// `header_only_decodes` counter tells a trusted header decode from no
/// detection at all, which is exactly the line [`SyncCause`] draws: no
/// trusted header is `SyncFailed`, a trusted header whose payload then failed
/// is [`AttemptOutcome::PayloadFailed`].
fn classify_streaming(decoded: &[&[u8]], any_trusted_header: bool, sent: &[u8]) -> AttemptOutcome {
    let mut delivered = false;
    let mut wrong = false;
    for got in decoded {
        if *got == sent {
            delivered = true;
        } else {
            wrong = true;
        }
    }
    if wrong {
        AttemptOutcome::WrongPayload
    } else if delivered {
        AttemptOutcome::Delivered
    } else if any_trusted_header {
        AttemptOutcome::PayloadFailed
    } else {
        AttemptOutcome::SyncFailed(SyncCause::NoDetection)
    }
}

/// Run a sweep of one receive path over `grid` and return one tally per point.
///
/// # Errors
///
/// Propagates [`run_attempt`]'s configuration errors.
#[allow(clippy::too_many_arguments)]
pub fn run_sweep(
    tx: &CoppaTransceiver,
    level: u8,
    payload_bytes: usize,
    channel: SweepChannel,
    rx_path: RxPath,
    grid: &[i32],
    trials: usize,
    seed_base: u64,
) -> anyhow::Result<Vec<PointTally>> {
    grid.iter()
        .enumerate()
        .map(|(pi, &snr)| {
            let mut tally = PointTally::new(f64::from(snr));
            for trial in 0..trials {
                let seed = attempt_seed(seed_base, pi, trials, trial);
                let (outcome, airtime) =
                    run_attempt(tx, level, payload_bytes, channel, rx_path, snr as f32, seed)?;
                tally.record(outcome, airtime, payload_bytes);
            }
            Ok(tally)
        })
        .collect()
}

/// Tail for the floor's one-second decode schedule. Together with the
/// randomized lead-in this includes a decode after the entire 8 s frame,
/// while keeping the capture below the production 10 s retention bound.
pub const FLOOR_TAIL_S: f64 = 1.25;

/// Run production floor receivers on exactly the same impaired capture per
/// seed, retaining separate tallies. Streaming has no trusted-header stage
/// counter, so non-delivery is conservatively SyncFailed.
///
/// # Errors
/// Rejects empty/unsorted grids, zero trials and empty receiver selections.
/// Production capacity/encoding failures are command errors, not FER.
pub fn run_floor_sweep(
    channel: SweepChannel,
    paths: &[RxPath],
    grid: &[i32],
    trials: usize,
    seed_base: u64,
) -> anyhow::Result<Vec<(RxPath, Vec<PointTally>)>> {
    anyhow::ensure!(
        !grid.is_empty() && grid.windows(2).all(|w| w[0] < w[1]),
        "SNR grid must be nonempty and strictly increasing"
    );
    anyhow::ensure!(
        trials > 0 && !paths.is_empty(),
        "trials and receivers must be nonempty"
    );
    let frame = coppa_protocol::modem::floor::FloorFrame::new();
    let mut results: Vec<_> = paths.iter().map(|&p| (p, Vec::new())).collect();
    for (pi, &snr) in grid.iter().enumerate() {
        let mut tallies = vec![PointTally::new(f64::from(snr)); paths.len()];
        for trial in 0..trials {
            let seed = attempt_seed(seed_base, pi, trials, trial);
            let (outcomes, airtime) = floor_attempt(&frame, channel, paths, snr as f32, seed)?;
            for (tally, outcome) in tallies.iter_mut().zip(outcomes) {
                tally.record(outcome, airtime, coppa_protocol::modem::floor::MAX_PAYLOAD);
            }
        }
        for ((_, points), tally) in results.iter_mut().zip(tallies) {
            points.push(tally);
        }
    }
    Ok(results)
}

fn floor_outcome(
    result: Result<&[u8], coppa_protocol::modem::floor::FloorError>,
    sent: &[u8],
) -> anyhow::Result<AttemptOutcome> {
    use coppa_protocol::modem::floor::FloorError;
    Ok(match result {
        Ok(got) if got == sent => AttemptOutcome::Delivered,
        Ok(_) => AttemptOutcome::WrongPayload,
        Err(FloorError::Acquisition) => AttemptOutcome::SyncFailed(SyncCause::NoDetection),
        Err(FloorError::Header) => AttemptOutcome::SyncFailed(SyncCause::HeaderDecode),
        Err(FloorError::Payload) => AttemptOutcome::PayloadFailed,
        Err(FloorError::Capacity) => anyhow::bail!("floor capacity/configuration error"),
    })
}

fn floor_attempt(
    frame: &coppa_protocol::modem::floor::FloorFrame,
    channel: SweepChannel,
    paths: &[RxPath],
    snr_db: f32,
    seed: u64,
) -> anyhow::Result<(Vec<AttemptOutcome>, usize)> {
    use coppa_protocol::modem::floor::{FloorStream, FRAME_SAMPLES, MAX_PAYLOAD};
    let mut rng = StdRng::seed_from_u64(seed);
    let payload: Vec<u8> = (0..MAX_PAYLOAD).map(|_| rng.random::<u8>()).collect();
    let clean = frame
        .encode(&payload)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    anyhow::ensure!(clean.len() == FRAME_SAMPLES, "unexpected floor airtime");
    let fading = match channel {
        SweepChannel::Awgn => None,
        SweepChannel::Poor => Some(WattersonPreset::Poor.config()),
    };
    let mut cable = VirtualCable::new(
        SAMPLE_RATE as f32,
        fading.as_ref(),
        Some(SnrProfile::constant(snr_db)),
        coppa_channel::mean_power(&clean),
        seed,
    );
    let jitter_s = f64::from(rng.random::<u16>()) / f64::from(u16::MAX) * 0.05;
    let mut capture = cable.idle(LEAD_IN_S + jitter_s).at_b;
    capture.extend(
        cable
            .transmit(coppa_channel::persistent::Direction::AToB, &clean)
            .at_b,
    );
    capture.extend(cable.idle(FLOOR_TAIL_S).at_b);
    let outcomes = paths
        .iter()
        .map(|path| match path {
            RxPath::Batch => {
                floor_outcome(frame.receive(&capture).as_deref().map_err(|e| *e), &payload)
            }
            RxPath::Streaming => {
                let mut stream = FloorStream::default();
                let mut decoded = Vec::new();
                for chunk in capture.chunks(STREAM_CHUNK_SAMPLES) {
                    decoded.extend(stream.push(chunk));
                }
                let slices: Vec<_> = decoded.iter().map(Vec::as_slice).collect();
                Ok(classify_streaming(&slices, false, &payload))
            }
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok((outcomes, clean.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_paths_share_capture() {
        let paths = [RxPath::Batch, RxPath::Streaming];
        let results = run_floor_sweep(SweepChannel::Awgn, &paths, &[30], 1, DEV_SEED_BASE).unwrap();
        assert_eq!(results.len(), 2);
        for (path, points) in results {
            assert!(paths.contains(&path));
            assert_eq!(points.len(), 1);
            assert_eq!(points[0].delivered, 1);
            assert_eq!(points[0].trials, 1);
            assert_eq!(
                points[0].airtime_samples,
                coppa_protocol::modem::floor::FRAME_SAMPLES as u64
            );
            assert_eq!(
                points[0].delivered_bytes,
                coppa_protocol::modem::floor::MAX_PAYLOAD as u64
            );
        }
    }

    #[test]
    fn floor_outcomes_are_conservative() {
        use coppa_protocol::modem::floor::FloorError;
        for (error, expected) in [
            (
                FloorError::Acquisition,
                AttemptOutcome::SyncFailed(SyncCause::NoDetection),
            ),
            (
                FloorError::Header,
                AttemptOutcome::SyncFailed(SyncCause::HeaderDecode),
            ),
            (FloorError::Payload, AttemptOutcome::PayloadFailed),
        ] {
            assert_eq!(floor_outcome(Err(error), b"sent").unwrap(), expected);
        }
        assert!(floor_outcome(Err(FloorError::Capacity), b"sent").is_err());
        assert_eq!(
            classify_streaming(&[], false, b"sent"),
            AttemptOutcome::SyncFailed(SyncCause::NoDetection)
        );
        // Full comparison rejects short payloads and matching prefixes with extra bytes.
        for got in [b"sen".as_slice(), b"sent extra".as_slice()] {
            assert_eq!(
                floor_outcome(Ok(got), b"sent").unwrap(),
                AttemptOutcome::WrongPayload
            );
        }
        for (grid, trials) in [(vec![], 1), (vec![2, 0], 1), (vec![0, 0], 1), (vec![0], 0)] {
            assert!(run_floor_sweep(
                SweepChannel::Awgn,
                &[RxPath::Batch],
                &grid,
                trials,
                DEV_SEED_BASE
            )
            .is_err());
        }
    }

    #[test]
    fn wrong_delivery_overrides_success() {
        for decoded in [
            vec![b"sent".as_slice(), b"wrong".as_slice()],
            vec![b"wrong".as_slice(), b"sent".as_slice()],
        ] {
            assert_eq!(
                classify_streaming(&decoded, false, b"sent"),
                AttemptOutcome::WrongPayload
            );
        }
    }

    fn tally(snr: f64, delivered: usize, sync: usize, payload: usize) -> PointTally {
        let mut t = PointTally::new(snr);
        for _ in 0..delivered {
            t.record(AttemptOutcome::Delivered, 100, 10);
        }
        for _ in 0..sync {
            t.record(AttemptOutcome::SyncFailed(SyncCause::NoDetection), 100, 10);
        }
        for _ in 0..payload {
            t.record(AttemptOutcome::PayloadFailed, 100, 10);
        }
        t
    }

    #[test]
    fn acceptance_grid_has_nine_points_from_minus_12_to_plus_4() {
        assert_eq!(acceptance_grid(), vec![-12, -10, -8, -6, -4, -2, 0, 2, 4]);
        assert!(snr_grid(0, 4, 0).is_empty());
        assert!(snr_grid(4, 0, 2).is_empty());
        assert_eq!(TRIALS_PER_POINT, 200);
    }

    #[test]
    fn seeds_are_locked_and_unique_per_attempt() {
        assert_eq!(DEV_SEED_BASE, 0x00C0_FFEE);
        assert_eq!(FINAL_SWEEP_SEED_BASE, 378_535_936);
        assert_eq!(FINAL_SESSION_SEED_BASE, 378_601_472);
        assert_eq!(attempt_seed(FINAL_SWEEP_SEED_BASE, 0, 200, 0), 0x1690_0000);
        assert_eq!(attempt_seed(FINAL_SWEEP_SEED_BASE, 1, 200, 0), 0x1690_00C8);
        let mut seen = std::collections::BTreeSet::new();
        for p in 0..9 {
            for t in 0..200 {
                assert!(seen.insert(attempt_seed(FINAL_SWEEP_SEED_BASE, p, 200, t)));
            }
        }
    }

    #[test]
    fn header_and_candidate_failures_count_as_sync_failed() {
        let mut t = PointTally::new(0.0);
        t.record(AttemptOutcome::SyncFailed(SyncCause::NoDetection), 10, 4);
        t.record(AttemptOutcome::SyncFailed(SyncCause::Mistimed), 10, 4);
        t.record(AttemptOutcome::SyncFailed(SyncCause::HeaderDecode), 10, 4);
        t.record(AttemptOutcome::PayloadFailed, 10, 4);
        t.record(AttemptOutcome::Delivered, 10, 4);
        assert_eq!(t.sync_failed, 3);
        assert_eq!(
            (t.sync_no_detection, t.sync_mistimed, t.sync_header_decode),
            (1, 1, 1)
        );
        assert_eq!(t.frame_errors(), 4);
        assert!(t.is_partition());
        assert!(!t.sync_share_ok());
        assert_eq!(t.delivered_bytes, 4);
    }

    #[test]
    fn legacy_receive_errors_map_conservatively() {
        let sent = [1u8, 2, 3];
        assert_eq!(
            AttemptOutcome::from_receive(Err(&ReceiveError::SyncFailed), &sent),
            AttemptOutcome::SyncFailed(SyncCause::NoDetection)
        );
        assert_eq!(
            AttemptOutcome::from_receive(Err(&ReceiveError::HeaderCorrupt), &sent),
            AttemptOutcome::SyncFailed(SyncCause::HeaderDecode)
        );
        assert_eq!(
            AttemptOutcome::from_receive(Err(&ReceiveError::CrcMismatch), &sent),
            AttemptOutcome::PayloadFailed
        );
        assert_eq!(
            AttemptOutcome::from_receive(Ok(&[1, 2, 3]), &sent),
            AttemptOutcome::Delivered
        );
        assert_eq!(
            AttemptOutcome::from_receive(Ok(&[1, 2, 4]), &sent),
            AttemptOutcome::WrongPayload
        );
    }

    #[test]
    fn zero_failures_satisfy_the_sync_share_clause() {
        let t = tally(4.0, 200, 0, 0);
        assert_eq!(t.frame_errors(), 0);
        assert!(t.sync_share_ok());
        // Exactly half is not "less than half".
        assert!(!tally(0.0, 0, 5, 5).sync_share_ok());
        assert!(tally(0.0, 0, 4, 5).sync_share_ok());
    }

    #[test]
    fn crossing_interpolates_between_bracketing_points() {
        let c = fifty_percent_crossing(&[(-10.0, 0.9), (-8.0, 0.3), (-6.0, 0.0)]).unwrap();
        let Crossing::At { db, all } = c else {
            panic!("expected a crossing");
        };
        // 0.9 -> 0.3 over 2 dB: 0.5 sits 2/3 of the way.
        assert!((db - (-10.0 + 2.0 * (0.4 / 0.6))).abs() < 1e-12);
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn non_monotone_curve_reports_highest_crossing_and_recrossings() {
        let curve = [
            (-12.0, 1.0),
            (-10.0, 0.4),
            (-8.0, 0.6),
            (-6.0, 0.2),
            (-4.0, 0.0),
        ];
        let Some(Crossing::At { db, all }) = fifty_percent_crossing(&curve) else {
            panic!("expected a crossing");
        };
        assert_eq!(all.len(), 3);
        assert!((db - (-8.0 + 2.0 * 0.25)).abs() < 1e-12);
    }

    #[test]
    fn all_success_and_all_failure_curves_are_bounded() {
        let below = fifty_percent_crossing(&[(-12.0, 0.1), (4.0, 0.0)]).unwrap();
        assert_eq!(
            below,
            Crossing::BelowGrid {
                upper_bound_db: -12.0
            }
        );
        assert_eq!(below.db(), Some(-12.0));
        let above = fifty_percent_crossing(&[(-12.0, 1.0), (4.0, 0.5)]).unwrap();
        assert_eq!(above, Crossing::NoPassingCrossing { all: vec![] });
        assert_eq!(above.db(), None);
        assert!(fifty_percent_crossing(&[]).is_none());
    }

    #[test]
    fn empty_points_and_sweeps_never_pass() {
        assert!(!PointTally::new(-12.0).is_partition());
        let v = evaluate(&[PointTally::new(-12.0), PointTally::new(4.0)], -2.0);
        assert_eq!(v.accounting_errors, vec![-12.0, 4.0]);
        assert!(!v.passes());
        assert!(!evaluate(&[], -2.0).passes());
    }

    #[test]
    fn harness_errors_are_returned_not_counted() {
        let tx = CoppaTransceiver::new(coppa_codec::ofdm::CoppaProfile::hf_narrow(), 1);
        assert!(run_attempt(&tx, 1, 70_000, SweepChannel::Awgn, RxPath::Batch, 30.0, 1).is_err());
        assert!(run_attempt(&tx, 1, 4_000, SweepChannel::Awgn, RxPath::Batch, 30.0, 1).is_err());
    }

    #[test]
    fn streaming_classification_keeps_sync_conservative() {
        let sent = [1u8, 2, 3];
        assert_eq!(
            classify_streaming(&[&sent], true, &sent),
            AttemptOutcome::Delivered
        );
        assert_eq!(
            classify_streaming(&[&[1, 2, 4]], true, &sent),
            AttemptOutcome::WrongPayload
        );
        assert_eq!(
            classify_streaming(&[&[1, 2]], true, &sent),
            AttemptOutcome::WrongPayload
        );
        // One wrong delivery alongside the correct frame still counts wrong:
        // the receiver handed the application bad bytes.
        assert_eq!(
            classify_streaming(&[&sent, &[1, 2, 4]], true, &sent),
            AttemptOutcome::WrongPayload
        );
        assert_eq!(
            classify_streaming(&[], true, &sent),
            AttemptOutcome::PayloadFailed
        );
        assert_eq!(
            classify_streaming(&[], false, &sent),
            AttemptOutcome::SyncFailed(SyncCause::NoDetection)
        );
    }

    #[test]
    fn verdict_checks_sync_share_only_above_the_crossing() {
        // Crossing between -10 (FER 1.0) and -8 (FER 0.25): -9.33 dB.
        let points = [
            tally(-10.0, 0, 200, 0), // all sync failures, below the crossing: allowed
            tally(-8.0, 150, 10, 40),
            tally(-6.0, 200, 0, 0),
        ];
        let v = evaluate(&points, AWGN_CROSSING_BAR_DB);
        assert!(v.passes(), "{v:?}");
        // Make the -8 dB point sync-dominated: the sweep must fail.
        let points = [
            tally(-10.0, 0, 200, 0),
            tally(-8.0, 150, 30, 20),
            tally(-6.0, 200, 0, 0),
        ];
        let v = evaluate(&points, AWGN_CROSSING_BAR_DB);
        assert_eq!(v.sync_share_violations, vec![-8.0]);
        assert!(!v.passes());
    }

    #[test]
    fn verdict_rejects_a_late_crossing_and_wrong_payloads() {
        let points = [tally(-8.0, 0, 0, 200), tally(-6.0, 200, 0, 0)];
        assert!(!evaluate(&points, AWGN_CROSSING_BAR_DB).passes());
        assert!(evaluate(&points, POOR_CROSSING_BAR_DB).passes());
        let mut bad = tally(-6.0, 199, 0, 0);
        bad.record(AttemptOutcome::WrongPayload, 100, 10);
        let v = evaluate(&[tally(-12.0, 200, 0, 0), bad], AWGN_CROSSING_BAR_DB);
        assert_eq!(v.wrong_payloads, 1);
        assert!(!v.passes());
    }

    #[test]
    fn csv_row_matches_the_header() {
        let row = tally(-8.0, 110, 20, 70).csv_row("floor", "batch", "awgn");
        assert_eq!(row.split(',').count(), CSV_HEADER.split(',').count());
        assert!(row.starts_with("floor,batch,awgn,-8,200,90,20,20,0,0,0,70,0,0.4500,"));
    }

    #[test]
    fn production_receiver_decodes_a_clean_attempt_through_the_cable() {
        let tx = CoppaTransceiver::new(coppa_codec::ofdm::CoppaProfile::hf_narrow(), 1);
        let (outcome, airtime) = run_attempt(
            &tx,
            1,
            16,
            SweepChannel::Awgn,
            RxPath::Batch,
            30.0,
            DEV_SEED_BASE,
        )
        .unwrap();
        assert_eq!(outcome, AttemptOutcome::Delivered);
        assert!(airtime > 0);
    }

    #[test]
    fn production_streaming_path_decodes_a_clean_attempt_through_the_cable() {
        let tx = CoppaTransceiver::new(coppa_codec::ofdm::CoppaProfile::hf_narrow(), 1);
        let (outcome, airtime) = run_attempt(
            &tx,
            1,
            16,
            SweepChannel::Awgn,
            RxPath::Streaming,
            30.0,
            DEV_SEED_BASE,
        )
        .unwrap();
        assert_eq!(outcome, AttemptOutcome::Delivered);
        assert!(airtime > 0);
    }

    #[test]
    fn both_rx_paths_agree_on_a_clean_poor_attempt() {
        let tx = CoppaTransceiver::new(coppa_codec::ofdm::CoppaProfile::hf_narrow(), 1);
        for rx_path in [RxPath::Batch, RxPath::Streaming] {
            let (outcome, _) = run_attempt(
                &tx,
                1,
                16,
                SweepChannel::Poor,
                rx_path,
                30.0,
                DEV_SEED_BASE + 7,
            )
            .unwrap();
            assert_eq!(outcome, AttemptOutcome::Delivered, "{rx_path:?}");
        }
    }
}
