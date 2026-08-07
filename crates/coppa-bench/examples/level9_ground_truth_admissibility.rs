//! Decode-independent level-9 admissibility oracle.
//!
//! `H(f,t) = sum sqrt(tap.power) * g_tap[t] * exp(-i 2 pi f tap.delay)` is taken
//! directly from the Watterson generator. Symbols are passed through that exact channel and decoded
//! with perfect CSI and sync, bypassing OFDM acquisition/equalization. `SPEED_LEVEL_MIN_CAPACITY`
//! is intentionally not used: it is calibrated on receiver measurements, not channel truth.
//!
//! Assumptions: CP absorbs all multipath; channel is static during one OFDM symbol; CSI and sync are
//! perfect. Thus Poor (96-sample delay versus VHF's 60-sample CP) is printed only as a
//! `FLAT_MODEL_ONLY` diagnostic, not an ISI-aware admissibility verdict. The reference noise uses
//! the harness's 3-kHz noise-band convention.

use coppa_bench::{
    ground_truth::{per_carrier_gain, AdmissibilityVerdict},
    metrics::wilson_ci95,
    scenario::SAMPLE_RATE,
};
use coppa_channel::watterson::{watterson_with_gains, WattersonPreset};
use coppa_codec::{
    ofdm::{interleaver::BlockInterleaver, pilots::CoppaPilotPattern, CoppaProfile},
    qam64::Qam64Mapper,
    traits::ConstellationMapper,
};
use coppa_protocol::fec::{
    ldpc::{
        pin_known_pad,
        rate_match::{rate_dematch, rate_match},
        NrLdpc,
    },
    scrambler::scramble,
};
use num_complex::Complex32;
use rand::{rngs::StdRng, Rng, RngExt, SeedableRng};

const LEVEL: u8 = 9;
const K_USED: usize = 1296;
const CODED_LEN: usize = 1944;
const PAYLOAD_BITS: usize = 158 * 8;
const DEFAULT_TRIALS: usize = 500;

fn gaussian(rng: &mut impl Rng) -> f32 {
    let u1 = (1.0f32 - rng.random::<f32>()).max(1e-12);
    let u2 = rng.random::<f32>();
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

fn trial(
    profile: &CoppaProfile,
    preset: WattersonPreset,
    snr_db: f32,
    seed: u64,
    ldpc: &NrLdpc,
) -> AdmissibilityVerdict {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut truth = vec![0u8; NrLdpc::INFO_LEN];
    for bit in &mut truth[..PAYLOAD_BITS] {
        *bit = rng.random::<u8>() & 1;
    }
    let mut info = truth.clone();
    scramble(&mut info);
    let matched = rate_match(&ldpc.encode(&info), K_USED, CODED_LEN, 0);
    let interleaver = BlockInterleaver::new(CODED_LEN, profile.data_carriers);
    let wire = interleaver.interleave(&matched);
    let mapper = Qam64Mapper;
    let symbols = mapper.map_bits(&wire);

    let samples_per_symbol = profile.fft_size + profile.cp_samples;
    let payload_symbols = symbols.len().div_ceil(profile.data_carriers);
    let frame_samples = (2 + payload_symbols) * samples_per_symbol;
    let (_, gains) = watterson_with_gains(
        &vec![0.0; frame_samples],
        SAMPLE_RATE as f32,
        &preset.config(),
        seed ^ 0x3333,
    );
    let pilot_pattern =
        CoppaPilotPattern::new(profile.total_active_carriers(), profile.pilot_carriers);
    // awgn_ref_seeded's nominal-SNR convention: full Nyquist noise is 8x the 3-kHz reference.
    let nv_ref = 10f32.powf(-snr_db / 10.0) * (SAMPLE_RATE as f32 / 2.0) / 3_000.0;
    let sigma = (nv_ref / 2.0).sqrt();
    let mut llrs = Vec::with_capacity(CODED_LEN);
    for (symbol_index, &symbol) in symbols.iter().enumerate() {
        let payload_symbol = symbol_index / profile.data_carriers;
        let within = symbol_index % profile.data_carriers;
        let grid_symbol = payload_symbol + 2;
        let carrier = pilot_pattern.data_indices(grid_symbol)[within];
        let frequency =
            (profile.first_active_bin() + carrier) as f32 * profile.carrier_spacing_hz();
        let time = ((grid_symbol * samples_per_symbol) + profile.cp_samples + profile.fft_size / 2)
            .min(frame_samples - 1);
        let h = per_carrier_gain(&gains, time, frequency);
        let received =
            h * symbol + Complex32::new(sigma * gaussian(&mut rng), sigma * gaussian(&mut rng));
        let h2 = h.norm_sqr().max(1e-9);
        for llr in mapper.demap_soft(received * h.conj() / h2, nv_ref / h2) {
            llrs.push(llr.clamp(-20.0, 20.0));
        }
    }
    llrs.truncate(CODED_LEN);
    let deinterleaved = interleaver.deinterleave(&llrs);
    let mut dematched = rate_dematch(&deinterleaved, K_USED, CODED_LEN, 0, NrLdpc::MOTHER_LEN);
    pin_known_pad(&mut dematched, PAYLOAD_BITS, K_USED, 64.0);
    let (_, mut decoded, converged) = ldpc.decode_soft(&dematched);
    scramble(&mut decoded);
    AdmissibilityVerdict {
        converged,
        payload_matches: decoded[..PAYLOAD_BITS] == truth[..PAYLOAD_BITS],
    }
}

fn main() {
    let trials = std::env::var("TRIALS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TRIALS);
    let snrs: Vec<f32> = std::env::var("SNRS")
        .unwrap_or_else(|_| "6,10,14,18,22,26,30".into())
        .split(',')
        .map(|s| s.parse().expect("SNRS must contain numbers"))
        .collect();
    let profiles = [
        ("vhf_wide", CoppaProfile::vhf_wide()),
        ("hf_standard", CoppaProfile::hf_standard()),
        ("hf_robust", CoppaProfile::hf_robust()),
    ];
    let ldpc = NrLdpc::new();
    for (profile_name, profile) in profiles {
        for (label, preset) in [
            ("Watterson-Good", WattersonPreset::Good),
            ("Watterson-Moderate", WattersonPreset::Moderate),
            ("Watterson-Poor-FLAT_MODEL_ONLY", WattersonPreset::Poor),
        ] {
            for &snr_db in &snrs {
                let admitted = (0..trials)
                    .filter(|&n| {
                        trial(&profile, preset, snr_db, 0xC0_0004 + n as u64, &ldpc).admissible()
                    })
                    .count();
                let failures = trials - admitted;
                let (failure_lo, failure_hi) = wilson_ci95(failures, trials);
                println!(
                    "=== {label} (TRIALS={trials}, snr_db={snr_db}, level={LEVEL}, profile={profile_name}) ===\nadmissible={admitted}/{trials} rate={:.4} wilson95=[{:.4},{:.4}]",
                    admitted as f64 / trials as f64,
                    1.0 - failure_hi,
                    1.0 - failure_lo,
                );
            }
        }
    }
    println!("Reading: this perfect-CSI isolated-FEC rate is an optimistic channel-admissibility bound; compare it with real decode rate at identical profile, preset, and SNR.");
}
