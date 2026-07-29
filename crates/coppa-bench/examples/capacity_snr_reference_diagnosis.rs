//! Systematic-debugging Phase 1 evidence-gathering: root-causing why `coppa_ml::recommend_speed_level`'s
//! same-frame accuracy under Watterson fading is weak (PR #57: correlation with the oracle only
//! 0.241 at a level-1 probe).
//!
//! Hypothesis under test: this is not (only) a noisy estimator, but a benchmark-methodology
//! artifact. Every existing Watterson bench (`mcs_calibration`, `closed_loop_arq`, `mcs_compare`,
//! `per_frame_link_diagnosis`, `adaptive_mcs_validation`) builds its received signal as
//! `coppa_channel::awgn_seeded(&faded, snr_db, seed)` -- referencing the injected noise power to
//! the FADED signal's OWN realized power (`awgn_with_rng`'s `signal_power` is computed from
//! whatever slice is passed in). `watterson.rs`'s own module doc warns against exactly this:
//! "Set noise from the CLEAN signal power ... never from the faded output, or fading cannot cost
//! SNR."
//!
//! Because `noise_power = mean_power(faded) / 10^(snr_db/10)`, the frame's own average (linear)
//! per-carrier SNR is pinned at exactly `snr_db` BY CONSTRUCTION, regardless of how deep that
//! trial's Rayleigh fade was -- only the within-frame frequency-selective spread of energy around
//! that fixed average can vary from trial to trial. `channel_capacity` (a per-carrier average of
//! the concave function `log2(1+SNR_k)`) is therefore predicted to be driven almost entirely by
//! that same spread (a Jensen's-inequality gap), which is exactly what `channel_selectivity`
//! already measures separately -- i.e. under this convention the two metrics are predicted to
//! collapse into one degenerate axis, carrying no real signal about the frame's overall/average
//! fade depth at all.
//!
//! This diagnostic measures, over many independent Watterson-Poor/Moderate realizations at a
//! fixed nominal SNR (24 dB, matching `closed_loop_arq`'s Poor-tail schedule point):
//!   1. `fade_ratio` = mean_power(faded) / mean_power(clean) -- the trial's real, overall fade depth.
//!   2. `capacity`/`selectivity` under the REAL (self-referenced) convention every existing bench uses.
//!   3. `capacity`/`selectivity` under a counterfactual CORRECT (clean-referenced, `awgn_ref_seeded`)
//!      convention, as a control.
//!
//! Prediction if the hypothesis holds: under (2), corr(fade_ratio, capacity) ~ 0 and
//! corr(capacity, selectivity) ~ -1 (one degenerate axis). Under (3), corr(fade_ratio, capacity)
//! should be strongly negative (real fade costs real SNR) and corr(capacity, selectivity) should
//! be markedly less extreme (two more-independent axes).
//!
//! This is a DIAGNOSTIC. It proposes no fix to the noise-injection convention itself (that would
//! mean re-baselining essentially every Watterson bench and calibrated table in this crate --
//! far outside this task's scope); it gathers evidence to localize the failing component.
//!
//! Run: `cargo run --release -p coppa-bench --example capacity_snr_reference_diagnosis`.

use coppa_bench::scenario::{mode_for_level, profile_by_name, SAMPLE_RATE};
use coppa_channel::watterson::{watterson, WattersonPreset};
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::{channel_capacity, channel_selectivity};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 400;
const SNR_DB: f32 = 24.0; // matches closed_loop_arq's Watterson-Poor tail nominal SNR
const LEVEL: u8 = 1; // BPSK 1/4: the passive-probe level RateLoop reads most when pinned low

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

fn pearson(xs: &[f32], ys: &[f32]) -> f32 {
    let n = xs.len() as f32;
    let mx = xs.iter().sum::<f32>() / n;
    let my = ys.iter().sum::<f32>() / n;
    let (mut cov, mut vx, mut vy) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..xs.len() {
        let (dx, dy) = (xs[i] - mx, ys[i] - my);
        cov += dx * dy;
        vx += dx * dx;
        vy += dy * dy;
    }
    cov / (vx.sqrt() * vy.sqrt()).max(1e-9)
}

struct Trial {
    fade_ratio: f32,
    cap_self: f32,
    sel_self: f32,
    cap_ref: f32,
    sel_ref: f32,
}

fn run(preset: WattersonPreset, label: &str) {
    let profile = profile_by_name("robust").unwrap();
    let modem = CoppaModem::new(profile.clone(), 1);
    let pfb = mode_for_level(LEVEL).unwrap().payload_bytes();

    // One fixed clean frame; only the channel realization varies across trials.
    let tx = CoppaTransceiver::new(profile, 1);
    let payload = vec![0x5Au8; pfb];
    let clean = tx
        .transmit(&make_header(LEVEL, pfb as u16), &payload)
        .expect("level-1 payload fits");
    let clean_power = coppa_channel::mean_power(&clean);

    let mut trials = Vec::with_capacity(TRIALS);
    for t in 0..TRIALS {
        let seed = 0x51DE_u64.wrapping_mul(t as u64 + 1);
        let faded = watterson(&clean, SAMPLE_RATE as f32, &preset.config(), seed ^ 0x3333);
        let fade_ratio = coppa_channel::mean_power(&faded) / clean_power;

        // (A) Self-referenced: the convention every existing Watterson bench uses.
        let rx_self = coppa_channel::awgn_seeded(&faded, SNR_DB, seed ^ 0x5555);
        let (cap_self, sel_self) = match modem.demodulate_frame(&rx_self) {
            Some((_h, _eq, nv, _ds)) => (channel_capacity(&nv), channel_selectivity(&nv)),
            None => (0.0, 0.0),
        };

        // (B) Clean-referenced control: the module doc's documented-correct convention.
        let rx_ref = coppa_channel::awgn_ref_seeded(
            &faded,
            SNR_DB,
            clean_power,
            SAMPLE_RATE as f32,
            seed ^ 0x5555,
        );
        let (cap_ref, sel_ref) = match modem.demodulate_frame(&rx_ref) {
            Some((_h, _eq, nv, _ds)) => (channel_capacity(&nv), channel_selectivity(&nv)),
            None => (0.0, 0.0),
        };

        trials.push(Trial {
            fade_ratio,
            cap_self,
            sel_self,
            cap_ref,
            sel_ref,
        });
    }

    let col = |f: fn(&Trial) -> f32| trials.iter().map(f).collect::<Vec<f32>>();
    let fade_ratio = col(|t| t.fade_ratio);
    let cap_self = col(|t| t.cap_self);
    let sel_self = col(|t| t.sel_self);
    let cap_ref = col(|t| t.cap_ref);
    let sel_ref = col(|t| t.sel_ref);

    println!("=== {label} (TRIALS={TRIALS}, snr_db={SNR_DB}, level={LEVEL}, profile=robust) ===");
    println!(
        "fade_ratio: mean={:.3} min={:.4} max={:.3}",
        fade_ratio.iter().sum::<f32>() / TRIALS as f32,
        fade_ratio.iter().cloned().fold(f32::MAX, f32::min),
        fade_ratio.iter().cloned().fold(0.0f32, f32::max),
    );
    println!(
        "[self-referenced, matches every existing bench] corr(fade_ratio, capacity) = {:.3}   corr(capacity, selectivity) = {:.3}",
        pearson(&fade_ratio, &cap_self),
        pearson(&cap_self, &sel_self),
    );
    println!(
        "[clean-referenced control, awgn_ref_seeded]      corr(fade_ratio, capacity) = {:.3}   corr(capacity, selectivity) = {:.3}",
        pearson(&fade_ratio, &cap_ref),
        pearson(&cap_ref, &sel_ref),
    );
    println!();
}

fn main() {
    run(WattersonPreset::Poor, "Watterson-Poor");
    run(WattersonPreset::Moderate, "Watterson-Moderate");
    println!(
        "Reading: if the hypothesis holds, self-referenced corr(fade_ratio, capacity) should sit\n\
         near 0 (overall fade depth invisible to the metric) while corr(capacity, selectivity)\n\
         sits near -1 (one degenerate axis, not two). The clean-referenced control should show the\n\
         opposite: a strongly negative corr(fade_ratio, capacity) (real fade costs real SNR) and a\n\
         markedly less extreme corr(capacity, selectivity)."
    );
}
