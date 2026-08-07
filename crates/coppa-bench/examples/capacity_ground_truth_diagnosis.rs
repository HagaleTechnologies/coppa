//! Systematic-debugging Phase 1 evidence-gathering (continuation of
//! `capacity_snr_reference_diagnosis.rs`, which ruled out a specific noise-referencing-convention
//! hypothesis -- see that file's module doc).
//!
//! Core question: is `coppa_ml::channel_capacity`/`channel_selectivity` (derived from the
//! receiver's pilot-based per-carrier noise-variance estimate `nv`) an ACCURATE estimator of the
//! real, instantaneous channel a frame passed through, or is a large part of PR #57's weak
//! same-frame `recommend_speed_level` accuracy (correlation with the oracle only 0.241 at a
//! level-1 probe) attributable to estimator noise/bias rather than inherent
//! decode-outcome unpredictability?
//!
//! Ground truth is computed directly from the Watterson channel model's OWN per-tap fading gains
//! (`coppa_channel::watterson::watterson_with_gains`), NOT from decode success or from the
//! receiver's pilot estimate: for each active OFDM subcarrier frequency `f_k`,
//! `H(f_k, t) = sum_taps sqrt(tap.power) * g_tap[t] * exp(-i*2*pi*f_k*tap.delay_s)`, sampled and
//! power-averaged over many time points spanning the whole frame (valid because HF Watterson
//! coherence time, ~1-10 s, is long relative to one frame -- see `watterson.rs`'s module doc, and
//! cross-checked below by comparing two disjoint time-sampling windows). Ground-truth per-carrier
//! SNR_k = `mean_t|H(f_k,t)|^2 * 10^(snr_db/10)` (the channel's taps are ensemble-normalized to
//! unit average power, so this reproduces the same nominal-SNR convention the receiver's estimator
//! is calibrated against). Ground-truth capacity/selectivity use the exact same
//! mean/std-of-`log2(1+SNR_k)` formulas `coppa_ml` uses, so they are directly comparable.
//!
//! Run: `cargo run --release -p coppa-bench --example capacity_ground_truth_diagnosis`.

use coppa_bench::ground_truth::{ground_truth_capacity, pearson};
use coppa_bench::scenario::{mode_for_level, profile_by_name, SAMPLE_RATE};
use coppa_channel::watterson::{watterson_with_gains, WattersonPreset};
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::{channel_capacity, channel_selectivity};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 300;
const SNR_DB: f32 = 24.0; // matches closed_loop_arq's Watterson-Poor tail nominal SNR
const LEVEL: u8 = 1; // BPSK: the passive-probe level RateLoop reads most when pinned low

fn make_header(level: u8, len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len: len,
        codewords: 1,
    }
}

/// The real receiver's own measured mean noise variance with NO fading (unit channel gain) at
/// `snr_db`, averaged over several seeds -- the calibration anchor `ground_truth` scales against.
fn measure_nv_ref(tx: &CoppaTransceiver, modem: &CoppaModem, clean: &[f32], snr_db: f32) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0usize;
    for s in 0..20u64 {
        let rx = coppa_channel::awgn_seeded(clean, snr_db, 0xF1A7 ^ s);
        if let Some((_h, _eq, nv, _ds)) = modem.demodulate_frame(&rx) {
            sum += nv.iter().sum::<f32>() / nv.len().max(1) as f32;
            n += 1;
        }
    }
    let _ = tx;
    sum / n.max(1) as f32
}

fn run(preset: WattersonPreset, label: &str) {
    let profile = profile_by_name("robust").unwrap();
    let modem = CoppaModem::new(profile.clone(), 1);
    let pfb = mode_for_level(LEVEL).unwrap().payload_bytes();
    let carrier_freqs: Vec<f32> = (0..profile.total_active_carriers())
        .map(|k| (profile.carrier_offset + 1 + k) as f32 * profile.carrier_spacing_hz())
        .collect();

    let tx = CoppaTransceiver::new(profile, 1);
    let payload = vec![0x5Au8; pfb];
    let clean = tx
        .transmit(&make_header(LEVEL, pfb as u16), &payload)
        .expect("level-1 payload fits");
    let nv_ref = measure_nv_ref(&tx, &modem, &clean, SNR_DB);
    println!("calibration: nv_ref (unit-gain, no fading) = {nv_ref:.5}");

    let (mut cap_meas, mut sel_meas) = (Vec::with_capacity(TRIALS), Vec::with_capacity(TRIALS));
    let (mut gt_cap_full, mut gt_sel_full) =
        (Vec::with_capacity(TRIALS), Vec::with_capacity(TRIALS));
    let (mut gt_cap_half, mut gt_sel_half) =
        (Vec::with_capacity(TRIALS), Vec::with_capacity(TRIALS));
    let mut n_decoded_nv = 0usize;

    for t in 0..TRIALS {
        let seed = 0x51DE_u64.wrapping_mul(t as u64 + 1);
        let (faded, gains) =
            watterson_with_gains(&clean, SAMPLE_RATE as f32, &preset.config(), seed ^ 0x3333);
        let rx = coppa_channel::awgn_seeded(&faded, SNR_DB, seed ^ 0x5555);

        if let Some((_h, _eq, nv, _ds)) = modem.demodulate_frame(&rx) {
            cap_meas.push(channel_capacity(&nv));
            sel_meas.push(channel_selectivity(&nv));
            n_decoded_nv += 1;
        } else {
            cap_meas.push(0.0);
            sel_meas.push(0.0);
        }

        let n = faded.len();
        // Two disjoint time windows -- if ground truth is stable across them (as the
        // long-coherence-time assumption predicts), the two should closely agree.
        let (c_full, s_full) = ground_truth_capacity(&gains, 0, n, &carrier_freqs, nv_ref);
        let (c_half, s_half) = ground_truth_capacity(&gains, n / 2, n, &carrier_freqs, nv_ref);
        gt_cap_full.push(c_full);
        gt_sel_full.push(s_full);
        gt_cap_half.push(c_half);
        gt_sel_half.push(s_half);
    }

    let stats = |xs: &[f32]| {
        let m = xs.iter().sum::<f32>() / xs.len() as f32;
        let v = xs.iter().map(|x| (x - m).powi(2)).sum::<f32>() / xs.len() as f32;
        (m, v.sqrt())
    };
    let (cm_mean, cm_std) = stats(&cap_meas);
    let (gc_mean, gc_std) = stats(&gt_cap_full);
    let (sm_mean, sm_std) = stats(&sel_meas);
    let (gs_mean, gs_std) = stats(&gt_sel_full);

    println!("=== {label} (TRIALS={TRIALS}, snr_db={SNR_DB}, level={LEVEL}, profile=robust) ===");
    println!("frames with a synced/demodulated header: {n_decoded_nv}/{TRIALS}");
    println!(
        "measured capacity: mean={cm_mean:.3} std={cm_std:.3}   ground-truth capacity: mean={gc_mean:.3} std={gc_std:.3}"
    );
    println!(
        "measured selectivity: mean={sm_mean:.3} std={sm_std:.3}   ground-truth selectivity: mean={gs_mean:.3} std={gs_std:.3}"
    );
    println!(
        "cross-check: corr(gt_capacity_full_frame, gt_capacity_second_half) = {:.3}  (near 1.0 confirms coherence-time assumption)",
        pearson(&gt_cap_full, &gt_cap_half)
    );
    println!(
        "corr(measured capacity,   ground-truth capacity)   = {:.3}",
        pearson(&cap_meas, &gt_cap_full)
    );
    println!(
        "corr(measured selectivity, ground-truth selectivity) = {:.3}",
        pearson(&sel_meas, &gt_sel_full)
    );
    println!(
        "corr(measured capacity, ground-truth selectivity)   = {:.3}  (cross-term sanity check)",
        pearson(&cap_meas, &gt_sel_full)
    );
    println!();
}

fn main() {
    run(WattersonPreset::Good, "Watterson-Good");
    run(WattersonPreset::Poor, "Watterson-Poor");
    run(WattersonPreset::Moderate, "Watterson-Moderate");
    println!(
        "Reading: a high corr(measured, ground-truth) means the receiver's pilot-based estimator\n\
         is an ACCURATE read of the real channel this frame passed through -- in which case PR\n\
         #57's weak same-frame oracle correlation is NOT an estimator-noise problem, and points\n\
         instead at a same-frame-average metric being an inherently weak predictor of discrete\n\
         decode outcomes (an information-granularity ceiling, not a fixable measurement bug). A low\n\
         correlation would instead point at the pilot-based estimate itself being inaccurate/noisy\n\
         relative to the real channel -- a measurement-layer bug worth root-causing further."
    );
}
