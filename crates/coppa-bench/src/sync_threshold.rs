//! COP-126: sync-threshold measurement for the two `SyncDetector` front ends.
//!
//! The FER bench (`runner::run_scenario`) decodes through the batch path,
//! which band-pass filters before sync, so it cannot see the streaming
//! receiver's sync front end. This module measures that front end directly:
//! the fraction of trials in which a detector's first candidate lands within
//! [`TIMING_TOLERANCE`] samples of the candidate the same front end finds on
//! the noise-free stream. That isolates sync from FEC. The SNR at which that
//! fraction crosses 50% is the sync threshold.
//!
//! Noise is full-band white noise from `coppa_channel::awgn_ref_seeded`, at
//! the bench's 3 kHz SNR convention, referenced to the mean power of the
//! whole frame. No RX filter is applied: this is what the streaming receiver
//! feeds its detector.
//!
//! `crates/coppa-bench/diagnostics/streaming_sync_threshold.rs` runs the full
//! sweep (BENCHMARKS.md, "COP-126"), and this module's test guards the
//! ticket's claim on every push.

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::sync_detector::SyncDetector;
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::speed_levels::max_payload_for_level;
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// A first candidate within this many samples of the front end's noise-free
/// candidate counts as a correct detection.
pub const TIMING_TOLERANCE: u64 = 60;

/// Chunk size the trials push into the detector.
const PUSH_CHUNK: usize = 512;

/// Which `SyncDetector` constructor a trial uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncFrontEnd {
    /// `SyncDetector::new`: the plateau search runs on the input as given.
    Raw,
    /// `SyncDetector::new_prefiltered`: the plateau search runs on a
    /// band-passed copy decimated by 4 (COP-126).
    Prefiltered,
}

impl SyncFrontEnd {
    /// A fresh detector of this kind for `profile`.
    pub fn detector(self, profile: CoppaProfile) -> SyncDetector {
        match self {
            Self::Raw => SyncDetector::new(profile, 1),
            Self::Prefiltered => SyncDetector::new_prefiltered(profile, 1),
        }
    }

    /// A short label for reports.
    pub fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Prefiltered => "prefiltered",
        }
    }
}

/// The `hf_standard` trial stream for `level`: 4 symbols of silence, the
/// first 6 symbols of a full frame (preamble, probe and header, which is all
/// sync needs) and 2 symbols of silence. Returns the stream, the mean power of
/// the whole frame (the SNR reference), and the frame's offset in the stream.
///
/// # Panics
///
/// Panics if `level` is not a speed level on the ladder (1-9).
pub fn sync_trial_stream(level: u8) -> (Vec<f32>, f32, u64) {
    let profile = CoppaProfile::hf_standard();
    let sym = profile.fft_size + profile.cp_samples;
    let tx = CoppaTransceiver::new(profile, 1);
    let payload_len =
        max_payload_for_level(level).unwrap_or_else(|| panic!("unknown speed level {level}"));
    let mut rng = StdRng::seed_from_u64(u64::from(level));
    let payload: Vec<u8> = (0..payload_len).map(|_| rng.random::<u8>()).collect();
    let header = CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len: payload_len as u16,
        codewords: 1,
    };
    let frame = tx
        .transmit(&header, &payload)
        .expect("a max-size payload fits its own level");
    let frame_power = coppa_channel::mean_power(&frame);

    let lead = 4 * sym;
    let mut stream = vec![0.0f32; lead];
    stream.extend_from_slice(&frame[..(6 * sym).min(frame.len())]);
    stream.extend(std::iter::repeat_n(0.0f32, 2 * sym));
    (stream, frame_power, lead as u64)
}

/// The first candidate's `frame_start` when `stream` is pushed into a fresh
/// `front` detector in 512-sample chunks.
pub fn first_candidate(front: SyncFrontEnd, stream: &[f32]) -> Option<u64> {
    let mut detector = front.detector(CoppaProfile::hf_standard());
    for chunk in stream.chunks(PUSH_CHUNK) {
        if let Some(c) = detector.push(chunk).first() {
            return Some(c.frame_start);
        }
    }
    None
}

/// How many of `trials` noisy copies of `level`'s trial stream, at `snr_db`
/// (3 kHz convention), the `front` detector times correctly. Trial `i` uses
/// noise seed `base_seed + i`. Returns 0 if the front end finds nothing on
/// the noise-free stream.
pub fn correct_detections(
    front: SyncFrontEnd,
    level: u8,
    snr_db: f32,
    trials: u32,
    base_seed: u64,
) -> u32 {
    let (stream, frame_power, _) = sync_trial_stream(level);
    let Some(reference) = first_candidate(front, &stream) else {
        return 0;
    };
    let sample_rate = CoppaProfile::hf_standard().sample_rate as f32;
    (0..trials)
        .filter(|&i| {
            let seed = base_seed.wrapping_add(u64::from(i));
            let noisy =
                coppa_channel::awgn_ref_seeded(&stream, snr_db, frame_power, sample_rate, seed);
            first_candidate(front, &noisy)
                .is_some_and(|start| start.abs_diff(reference) <= TIMING_TOLERANCE)
        })
        .count() as u32
}

/// The SNR at which `rates` (detection fractions, one per entry of `snrs`,
/// both in ascending SNR order) first reaches `target`, linearly
/// interpolated between the two grid points that bracket it. `None` if the
/// rate never reaches `target`, or if the slices differ in length.
pub fn crossing_db(snrs: &[f32], rates: &[f32], target: f32) -> Option<f32> {
    if snrs.len() != rates.len() {
        return None;
    }
    let i = rates.iter().position(|&r| r >= target)?;
    if i == 0 {
        return Some(snrs[0]);
    }
    let (s0, s1, r0, r1) = (snrs[i - 1], snrs[i], rates[i - 1], rates[i]);
    Some(s0 + (target - r0) / (r1 - r0) * (s1 - s0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// COP-126 scenario "Sync margin is recovered": at levels 1 and 2 on
    /// full-band AWGN, the raw detector's 50% point lies above 12 dB and the
    /// pre-filtered detector's lies below 6 dB, so the pre-filter moves the
    /// sync threshold down by more than 6 dB. Detection rate rises with SNR,
    /// so the two counts bound the two 50% points.
    #[test]
    fn prefilter_moves_the_sync_threshold_down_by_at_least_6_db_at_levels_1_and_2() {
        const TRIALS: u32 = 20;
        const SEED: u64 = 0x00C0_FFEE;
        for level in [1u8, 2] {
            let raw = correct_detections(SyncFrontEnd::Raw, level, 12.0, TRIALS, SEED);
            let pre = correct_detections(SyncFrontEnd::Prefiltered, level, 6.0, TRIALS, SEED);
            assert!(
                raw <= TRIALS / 2,
                "level {level}: raw detector found {raw}/{TRIALS} at 12 dB, so its 50% \
                 point is not above 12 dB"
            );
            assert!(
                pre >= 18,
                "level {level}: prefiltered detector found {pre}/{TRIALS} at 6 dB, so its \
                 50% point is not clearly below 6 dB"
            );
        }
    }

    #[test]
    fn trial_stream_starts_with_silence_and_finds_a_clean_candidate() {
        let (stream, power, offset) = sync_trial_stream(1);
        assert!(power > 0.0);
        assert!(stream[..offset as usize].iter().all(|&s| s == 0.0));
        let raw = first_candidate(SyncFrontEnd::Raw, &stream).expect("raw clean candidate");
        let pre =
            first_candidate(SyncFrontEnd::Prefiltered, &stream).expect("prefiltered candidate");
        assert_eq!(raw, pre, "both front ends time a clean frame identically");
    }

    #[test]
    fn crossing_db_interpolates_between_bracketing_points() {
        let snrs = [1.0, 2.0, 3.0];
        let rates = [0.0, 0.4, 0.8];
        let x = crossing_db(&snrs, &rates, 0.5).expect("crosses");
        assert!((x - 2.25).abs() < 1e-6, "{x}");
        assert_eq!(crossing_db(&snrs, &rates, 0.9), None);
        assert_eq!(crossing_db(&snrs, &rates, 0.0), Some(1.0));
        assert_eq!(crossing_db(&snrs, &rates[..2], 0.5), None);
    }
}
