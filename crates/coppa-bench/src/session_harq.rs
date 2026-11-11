//! COP-186: paired per-level retransmission-rescue measurement backing
//! `coppa_protocol::arq::RV_SCHEDULE`.
//!
//! The `session_harq` example (`diagnostics/session_harq.rs`) drives these
//! helpers to measure, per HF speed level, how many failed RV0 frames each
//! retransmission order rescues: Chase (RV0 again, combined) and all six
//! orders of `{1, 2, 3}` for retransmissions 1-3, over identical channel
//! draws (a paired comparison, so arms differ only in the RVs they send).
//! The selection rule [`select_row`] then turns a level's arm results into
//! the shipped `RV_SCHEDULE` row; `BENCHMARKS.md`'s `session_harq` section
//! records the run those rows came from.
//!
//! Frames are session-like per the COP-168 plan's method: a random payload of
//! `max_multi_payload_for_level(level, cw) - 21` bytes (the 21 bytes a
//! session `TransportPdu::Reliable` header plus margin costs), 1200 zero
//! samples appended after the frame (PTT tail), Watterson fading then AWGN
//! at the HF 3 kHz noise convention, and a fresh channel seed per physical
//! transmission. SNR is referenced to the clean frame's own power (measured
//! before the tail is appended, so the zeros do not dilute it).

use coppa_channel::awgn_ref_seeded;
use coppa_channel::watterson::{watterson_preset, WattersonPreset};
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// Sample rate of every Coppa profile (48 kHz, AGENTS.md).
pub const SAMPLE_RATE: f32 = 48_000.0;

/// Zero samples appended after each frame before the channel, standing in
/// for a session's PTT tail so late-frame multipath energy is not truncated.
pub const SESSION_TAIL_SAMPLES: usize = 1200;

/// Bytes subtracted from `max_multi_payload_for_level` for the session
/// framing a real sender adds around the application payload.
pub const PAYLOAD_HEADROOM_BYTES: usize = 21;

/// The six retransmission orders of `{1, 2, 3}` the measurement arms run,
/// in a fixed canonical listing order (the selection rule's final
/// tie-breaker refers to this order).
pub const ORDERS: [[u8; 3]; 6] = [
    [1, 2, 3],
    [1, 3, 2],
    [2, 1, 3],
    [2, 3, 1],
    [3, 1, 2],
    [3, 2, 1],
];

/// 3GPP's retransmission order (`rv_for_attempt`'s `[0, 2, 3, 1]` cycle) as
/// a permutation of `{1, 2, 3}`: RV2, then RV3, then RV1.
pub const THREE_GPP_ORDER: [u8; 3] = [2, 3, 1];

/// Session-like payload for `level`/`codewords`: random bytes filling the
/// frame to `max_multi_payload_for_level - 21` (deterministic in `seed`).
pub fn session_payload(level: u8, codewords: u8, seed: u64) -> Vec<u8> {
    let len = coppa_protocol::modem::speed_levels::max_multi_payload_for_level(level, codewords)
        .expect("level exists")
        - PAYLOAD_HEADROOM_BYTES;
    let mut rng = StdRng::seed_from_u64(seed);
    (0..len).map(|_| rng.random::<u8>()).collect()
}

/// PHY header of one session-frame transmission attempt. `payload_len` must
/// equal the payload's length: the receiver slices codewords and verifies the
/// CRC against the wire header's value.
pub fn session_header(
    level: u8,
    codewords: u8,
    seq: u8,
    rv: u8,
    payload_len: usize,
) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: rv, // low 2 bits = RV
        speed_level: level,
        seq_num: seq,
        payload_len: payload_len as u16,
        codewords,
    }
}

/// One physical transmission's channel: optional Watterson fading then AWGN
/// at `snr_db` (3 kHz reference, `p_clean` measured on the clean frame before
/// the tail), seeded per physical transmission so every arm of a trial sees
/// identical draws for the same `(trial, attempt)` pair.
pub fn session_channel(
    clean: &[f32],
    preset: Option<WattersonPreset>,
    snr_db: f32,
    p_clean: f32,
    seed: u64,
) -> Vec<f32> {
    let mut with_tail = Vec::with_capacity(clean.len() + SESSION_TAIL_SAMPLES);
    with_tail.extend_from_slice(clean);
    with_tail.resize(with_tail.len() + SESSION_TAIL_SAMPLES, 0.0);
    match preset {
        Some(preset) => {
            let faded = watterson_preset(
                &with_tail,
                SAMPLE_RATE,
                preset,
                seed ^ 0x3333_3333_3333_3333,
            );
            awgn_ref_seeded(
                &faded,
                snr_db,
                p_clean,
                SAMPLE_RATE,
                seed ^ 0x5555_5555_5555_5555,
            )
        }
        None => awgn_ref_seeded(
            &with_tail,
            snr_db,
            p_clean,
            SAMPLE_RATE,
            seed ^ 0x5555_5555_5555_5555,
        ),
    }
}

/// Channel seed for physical transmission `attempt` (0 = the original RV0
/// frame) of `trial`, so a given `(trial, attempt)` draws the identical
/// channel regardless of which arm is transmitting.
pub fn channel_seed(trial: u64, attempt: u32) -> u64 {
    0x5E55_0000_0000_0000u64
        .wrapping_add(trial.wrapping_mul(1_000))
        .wrapping_add(u64::from(attempt))
}

/// Mean power of the clean frame (pre-tail), the SNR reference.
pub fn frame_power(clean: &[f32]) -> f32 {
    coppa_channel::mean_power(clean)
}

/// Whether `rx` is a CRC-passing decode of exactly `payload`.
pub fn payload_matches(
    rx: &Result<(CoppaHeader, Vec<u8>, u8), coppa_protocol::modem::transceiver::ReceiveError>,
    payload: &[u8],
) -> bool {
    matches!(rx, Ok((_, data, _)) if data.as_slice() == payload)
}

/// One measurement arm's rescue tally over a level's failed RV0 frames.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArmResult {
    /// Failed RV0 frames rescued by retransmission 1 / by 1-2 / by 1-3
    /// (cumulative), per arm RV order.
    pub rescued: [usize; 3],
    /// Discordant pairs against Chase at each retransmission step:
    /// `(arm rescued, Chase did not)` and `(arm did not, Chase rescued)`.
    pub discordant: [(usize, usize); 3],
}

/// The selection rule (COP-168 plan, Implementation Approach item 4): keep
/// the order with the best first-retransmission rescue; ties within one
/// frame go to the best cumulative rescue after retransmission 2; remaining
/// ties go to the order closest to 3GPP's `[0, 2, 3, 1]` (most positions
/// equal to `[2, 3, 1]`), then to the earlier entry of [`ORDERS`].
///
/// `rescued1`/`rescued2` are an order's rescue counts after retransmission 1
/// and after retransmissions 1-2. Deterministic: equal tallies always resolve
/// to the same row.
pub fn select_row(rescued1: &[usize; 6], rescued2: &[usize; 6]) -> [u8; 4] {
    let mut best = 0usize;
    for cand in 1..ORDERS.len() {
        best = better_of(best, cand, rescued1, rescued2);
    }
    let order = ORDERS[best];
    [0, order[0], order[1], order[2]]
}

/// Index of the better of two order entries under the selection rule.
fn better_of(a: usize, b: usize, rescued1: &[usize; 6], rescued2: &[usize; 6]) -> usize {
    // Rule 1: best first-retransmission rescue.
    if rescued1[a] != rescued1[b] {
        return if rescued1[a] > rescued1[b] { a } else { b };
    }
    // Rule 2 (tie within one frame): best after the second retransmission.
    if rescued2[a] != rescued2[b] {
        return if rescued2[a] > rescued2[b] { a } else { b };
    }
    // Rule 3: order closest to 3GPP's [0, 2, 3, 1] == [2, 3, 1] over {1,2,3}.
    let matches = |i: usize| {
        ORDERS[i]
            .iter()
            .zip(THREE_GPP_ORDER.iter())
            .filter(|(x, y)| x == y)
            .count()
    };
    if matches(a) != matches(b) {
        return if matches(a) > matches(b) { a } else { b };
    }
    // Final tie: the earlier entry of the canonical ORDERS listing.
    a.min(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_codec::ofdm::CoppaProfile;
    use coppa_protocol::arq::RV_SCHEDULE;
    use coppa_protocol::modem::transceiver::CoppaTransceiver;

    #[test]
    fn select_row_prefers_the_best_first_retransmission() {
        // Order [3, 1, 2] (index 4) rescues most after retransmission 1.
        let mut r1 = [3usize; 6];
        r1[4] = 10;
        let mut r2 = r1;
        r2[4] = 12;
        assert_eq!(select_row(&r1, &r2), [0, 3, 1, 2]);
    }

    #[test]
    fn select_row_breaks_first_ties_on_the_second_retransmission() {
        let r1 = [7usize; 6];
        let mut r2 = [9usize; 6];
        r2[2] = 11; // order [2, 1, 3]
        assert_eq!(select_row(&r1, &r2), [0, 2, 1, 3]);
    }

    #[test]
    fn select_row_breaks_remaining_ties_toward_3gpp_then_listing_order() {
        let r1 = [5usize; 6];
        let r2 = [8usize; 6];
        // [2, 3, 1] (index 3) shares three positions with THREE_GPP_ORDER.
        assert_eq!(select_row(&r1, &r2), [0, 2, 3, 1]);
        // With 3GPP's own order knocked out of the tie (rescued fewer), the
        // closest remaining is [1, 3, 2] (one position, RV3 in the middle)
        // ahead of the zero-position orders, whatever ORDERS' listing order.
        let mut r1b = [5usize; 6];
        let mut r2b = [8usize; 6];
        r1b[3] = 4;
        r2b[3] = 8;
        assert_eq!(select_row(&r1b, &r2b), [0, 1, 3, 2]);
        // Only when 3GPP-closeness also ties does the ORDERS listing order
        // decide: [1, 3, 2] and [3, 2, 1] each share one position and, with
        // every other order knocked out of the tie, the earlier-listed
        // [1, 3, 2] wins.
        let mut r1c = [5usize; 6];
        let r2c = [8usize; 6];
        for out in [0usize, 2, 3, 4] {
            r1c[out] = 4;
        }
        assert_eq!(select_row(&r1c, &r2c), [0, 1, 3, 2]);
    }

    #[test]
    fn select_row_is_deterministic_on_full_ties() {
        // Everything equal: the order closest to 3GPP's wins, so the same
        // tallies always produce the same row.
        let r1 = [0usize; 6];
        let r2 = [0usize; 6];
        assert_eq!(select_row(&r1, &r2), [0, 2, 3, 1]);
    }

    /// AC4 guard: at level 3 on AWGN 3 dB -- the operating point where the
    /// COP-129/COP-168 probes measured 3GPP's RV2 first retransmission
    /// rescuing 0 of 36 failed RV0 frames while a Chase repeat rescued 21 --
    /// the SHIPPED schedule's first retransmission must rescue at least one
    /// failed RV0 frame. 40 seeds, paired draws, one transceiver, buffer
    /// evicted between seeds (the fixed-harness convention).
    #[test]
    fn shipped_schedule_rescues_at_level_3_awgn() {
        const LEVEL: u8 = 3;
        const SNR_DB: f32 = 3.0;
        const SEEDS: u64 = 40;
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile, 1);
        let mut rescued = 0usize;
        let mut failures = 0usize;
        for seed in 0..SEEDS {
            let payload = session_payload(LEVEL, 1, 0xA1B2_0000 + seed);
            let clean0 = tx
                .transmit(&session_header(LEVEL, 1, 0, 0, payload.len()), &payload)
                .expect("payload within level 3 capacity");
            let p = frame_power(&clean0);
            let noisy0 = session_channel(&clean0, None, SNR_DB, p, channel_seed(seed, 0));
            let ok0 = payload_matches(&tx.receive(&noisy0), &payload);
            if ok0 {
                // A passing decode evicts the soft buffer itself; a failing
                // one leaves this seed's RV0 LLRs in place, which is exactly
                // the state a retransmission combines with.
                continue;
            }
            failures += 1;
            // The shipped first retransmission on its paired draw.
            let rv1 = coppa_protocol::arq::rv_for_session_attempt(LEVEL, 1);
            assert_ne!(rv1, 0, "a retransmission never repeats RV0");
            let clean1 = tx
                .transmit(&session_header(LEVEL, 1, 0, rv1, payload.len()), &payload)
                .expect("retransmission within capacity");
            let noisy1 = session_channel(&clean1, None, SNR_DB, p, channel_seed(seed, 1));
            if payload_matches(&tx.receive(&noisy1), &payload) {
                rescued += 1;
            }
            tx.harq_evict(0);
        }
        assert!(
            failures > 0,
            "level 3 at 3 dB AWGN must fail some RV0 frames for the guard to mean anything"
        );
        assert!(
            rescued >= 1,
            "shipped schedule row {:?} rescued {rescued} of {failures} failed RV0 frames \
             at level 3 on AWGN 3 dB (the COP-129 probe measured RV2-first at 0 of 36)",
            RV_SCHEDULE[LEVEL as usize]
        );
    }
}
