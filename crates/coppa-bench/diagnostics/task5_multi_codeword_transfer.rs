//! Phase 3 Task 5 bench (job item 2): 10 kB bulk-transfer time at levels 2/6,
//! comparing the pre-Task-5 baseline (one LDPC codeword per frame, one ARQ
//! turnaround between every frame) against Task 5's multi-codeword framing (up
//! to 8 codewords back-to-back in one frame, same per-frame turnaround but far
//! fewer frames needed for the same total payload).
//!
//! The Phase-0-era arithmetic (single-codeword frames, one 150 ms half-duplex
//! turnaround per frame -- `coppa_protocol::arq::DEFAULT_TURNAROUND`, decision 4)
//! showed ~61 s for a 10 kB transfer at level 6; this task's target is <= 40 s.
//!
//! # Timing model
//!
//! `per_frame_time = frame_airtime_s(level) + turnaround` (a single
//! half-duplex turnaround per frame -- the ACK itself is treated as
//! comparatively negligible airtime, not modeled as its own full frame here;
//! this is a simplified bulk-transfer estimate in the same spirit as the
//! "Phase-0-era arithmetic" this task's own brief cites as ~61 s, not a
//! full ARQ simulation). `total_time = frames_needed * per_frame_time`. This
//! is deliberately simpler than `coppa_protocol::arq::rto_floor`'s `burst +
//! 2*turnaround + ack_airtime` RTO-floor formula (which, applied naively per
//! data frame with a full level-2 ACK frame's airtime added on top of a much
//! shorter payload frame, would swamp the comparison in ACK overhead rather
//! than isolating the thing Task 5 actually changes: frame *count*). The
//! baseline term reuses `coppa_protocol::modem::airtime::frame_airtime_s`
//! directly (it only knows about one codeword per frame). The multi-codeword
//! term can't reuse that function unchanged (it has no codeword-count
//! parameter), so this bench measures the REAL multi-codeword frame airtime by
//! actually calling `CoppaTransceiver::transmit` at max per-frame capacity and
//! dividing the returned sample count by the profile's sample rate -- an
//! honest, real measurement rather than a formula extrapolated from the
//! single-codeword case.

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::arq::DEFAULT_TURNAROUND;
use coppa_protocol::modem::airtime::frame_airtime_s;
use coppa_protocol::modem::speed_levels::{max_multi_payload_for_level, max_payload_for_level};
use coppa_protocol::modem::transceiver::MAX_CODEWORDS;
use coppa_protocol::modem::CoppaTransceiver;

/// 10 kB transfer, matching the job item's own wording.
const TOTAL_BYTES: usize = 10 * 1024;

fn make_header(speed_level: u8, payload_len: u16, codewords: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level,
        seq_num: 0,
        payload_len,
        codewords,
    }
}

/// Baseline: one codeword per frame, one turnaround per frame -- see this
/// file's module doc for the timing model.
fn baseline_transfer_time_s(level: u8, profile: &CoppaProfile) -> f64 {
    let per_frame = max_payload_for_level(level).expect("valid level");
    let frames_needed = TOTAL_BYTES.div_ceil(per_frame);
    let airtime = frame_airtime_s(level, profile).expect("valid level");
    frames_needed as f64 * (airtime + DEFAULT_TURNAROUND.as_secs_f64())
}

/// Task 5: up to `MAX_CODEWORDS` codewords per frame, same per-frame
/// turnaround, but far fewer frames needed for the same total payload.
fn multi_codeword_transfer_time_s(level: u8, profile: &CoppaProfile) -> f64 {
    let per_frame = max_multi_payload_for_level(level, MAX_CODEWORDS).expect("valid level");
    let frames_needed = TOTAL_BYTES.div_ceil(per_frame);

    // Real, measured multi-codeword frame airtime (not a formula extrapolation):
    // build a full-capacity MAX_CODEWORDS-codeword frame and time it for real.
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let payload = vec![0xA5u8; per_frame];
    let header = make_header(level, per_frame as u16, MAX_CODEWORDS);
    let samples = tx
        .transmit(&header, &payload)
        .expect("full-capacity multi-codeword frame must fit its own capacity");
    let multi_airtime_s = samples.len() as f64 / profile.sample_rate as f64;

    frames_needed as f64 * (multi_airtime_s + DEFAULT_TURNAROUND.as_secs_f64())
}

fn main() {
    let profile = CoppaProfile::hf_standard();
    println!("Phase 3 Task 5: 10 kB transfer time, single- vs multi-codeword framing");
    println!(
        "(turnaround = {:?} per frame; Phase-0-era baseline was ~61s at level 6)\n",
        DEFAULT_TURNAROUND
    );
    println!("| Level | Baseline (1 codeword/frame) | Multi-codeword (<= {MAX_CODEWORDS}/frame) | Target |");
    println!("|-------|------------------------------|--------------------------------|--------|");

    for level in [2u8, 6u8] {
        let baseline = baseline_transfer_time_s(level, &profile);
        let multi = multi_codeword_transfer_time_s(level, &profile);
        println!("| {level} | {baseline:.1} s | {multi:.1} s | <= 40 s |");
    }

    println!("\nJob item 2's own gate: level 6 multi-codeword transfer time must be <= 40 s");
    let level6_multi = multi_codeword_transfer_time_s(6, &profile);
    assert!(
        level6_multi <= 40.0,
        "level 6 multi-codeword 10 kB transfer time {level6_multi:.1}s exceeds the 40s target"
    );
    println!("PASS: level 6 multi-codeword transfer time = {level6_multi:.1}s <= 40s");
}
