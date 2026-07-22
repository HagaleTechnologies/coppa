//! Root-cause isolation for the level-dependence PR #51 confirmed in AWGN
//! (`capacity_metric_level_bias.rs`: +1.581 bits/s/Hz level1->level10, 13x its
//! own noise bound, while all three Watterson fading channels read within
//! noise). That result redirected suspicion away from coherence-time/fading
//! and toward "something in the per-level measurement path itself" -- this
//! bench tests one concrete candidate: `SPEED_LEVELS[level].papr_target_db`
//! (6.0 dB at BPSK up to 14.0 dB at 64-QAM, see `coppa_modem.rs`) is a
//! DELIBERATELY level-dependent hard-clip threshold applied to the WHOLE
//! frame's time-domain samples (preamble, probe, header, AND payload pilots
//! all get clipped at the same single per-frame threshold -- see
//! `CoppaModem::modulate_mapped`'s TX-conditioning chain and `papr_clip`'s
//! doc). A tighter clip (closer to RMS) at low levels distorts the probe/
//! pilot subcarriers the delay-domain estimator and Kalman tracker read
//! their noise variance from -- an SNR-INDEPENDENT self-noise floor that
//! only becomes visible once thermal AWGN shrinks enough to stop masking it
//! (i.e. at high SNR), matching exactly where PR #51 saw AWGN's capacity
//! trend clear its noise bound.
//!
//! # Hypothesis (falsifiable)
//!
//! If the differential `papr_target_db` per level is the mechanism, then
//! forcing every level through the SAME clip target (holding modulation
//! order, pilot pattern, and everything else exactly as production uses it)
//! should collapse most of the level1->level10 capacity gap, in BOTH
//! directions: giving low levels the loose 14.0 dB target production only
//! gives level 10 should raise their capacity toward level 10's; forcing
//! level 10 through the harsh 6.0 dB target production only gives levels
//! 1-2 should pull level 10's capacity down toward theirs. If the gap
//! persists similarly under a UNIFORM clip target, that falsifies this
//! mechanism and points elsewhere (e.g. an equalization/noise-estimation
//! effect intrinsic to modulation order, independent of clipping).
//!
//! # Method
//!
//! For each level, build ONE fixed set of constellation-mapped payload
//! symbols (pseudorandom bits through that level's real mapper, exactly
//! `CODED_BLOCK_LEN.div_ceil(bits_per_symbol)` symbols -- the same count
//! `CoppaTransceiver::transmit` would produce for a single codeword) and
//! modulate it three ways, varying ONLY `papr_target_db`:
//!   - `natural`: this level's real `SPEED_LEVELS` target (what production
//!     ships).
//!   - `uniform_loose`: every level forced through 14.0 dB (level 10's real,
//!     loosest target).
//!   - `uniform_harsh`: every level forced through 6.0 dB (level 1/2's real,
//!     harshest target).
//!
//! Each of the three signals is then AWGN-faded (many seeds, several SNRs)
//! and demodulated via the real `CoppaModem::demodulate_frame`, exactly as
//! `capacity_metric_level_bias.rs` does -- same trials count, same channel
//! helper, same significance-check convention.
//!
//! Output: `DATA <condition> <snr> <level> <mean_capacity> <std_capacity>`
//! followed by a per-condition SUMMARY line (level1->level10 mean capacity
//! gap, averaged across the SNR grid, with a 2*SE significance bound -- same
//! convention as `capacity_metric_level_bias.rs`).
//!
//! Diagnose-only: does not touch `coppa_ml`, `RateLoop`, or the production
//! `papr_target_db` table.

use coppa_bench::scenario::profile_by_name;
use coppa_codec::ofdm::coppa_modem::{CoppaModem, SPEED_LEVELS};
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::channel_capacity;
use coppa_protocol::modem::speed_level_components;
use num_complex::Complex32;

const TRIALS: usize = 40;
const CODED_BLOCK_LEN: usize = 1944;

/// The real levels `SPEED_LEVELS`/`speed_level_components` support (8 is
/// reserved), ascending -- same set `capacity_metric_level_bias.rs` sounds.
const LEVELS: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 9, 10];

const UNIFORM_LOOSE_DB: f32 = 14.0; // level 10's real target
const UNIFORM_HARSH_DB: f32 = 6.0; // level 1/2's real target

/// (condition name, per-level papr_target_db chooser).
type PaprCondition = (&'static str, fn(u8) -> f32);

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

/// Deterministic xorshift64* bit stream -- no crypto/statistical properties
/// needed, just a reproducible pseudorandom {0,1} sequence so the mapped
/// symbols exercise a realistic (non-constant) constellation point spread,
/// same as real coded/scrambled payload bits would.
fn pseudorandom_bits(seed: u64, n: usize) -> Vec<u8> {
    let mut state = seed | 1;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state & 1) as u8
        })
        .collect()
}

/// Build this level's payload symbols: one LDPC-codeword's worth of coded
/// bits (`CODED_BLOCK_LEN`, codewords=1, same as `CoppaTransceiver::transmit`
/// computes for a single codeword) mapped through the level's REAL
/// constellation mapper. Fixed per level (same `seed`) so all three PAPR
/// conditions modulate byte-for-byte identical symbol content -- content is
/// held constant, only `papr_target_db` varies.
fn build_symbols(level: u8) -> Vec<Complex32> {
    let (mapper, _rate) = speed_level_components(level).expect("valid level");
    let bps = mapper.bits_per_symbol();
    let n_symbols = CODED_BLOCK_LEN.div_ceil(bps);
    let n_bits = n_symbols * bps;
    let bits = pseudorandom_bits(
        0xB17D_5EED_u64 ^ (level as u64).wrapping_mul(0x9E37_79B9),
        n_bits,
    );
    mapper.map_bits(&bits)
}

/// Mean and population standard deviation of a slice.
fn mean_std(xs: &[f32]) -> (f32, f32) {
    let n = xs.len() as f32;
    let mean = xs.iter().sum::<f32>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
    (mean, var.sqrt())
}

/// Modulate `level`'s fixed symbol set at `papr_target_db`, AWGN-fade it
/// `TRIALS` times at `snr`, and return each successfully-demodulated trial's
/// `channel_capacity`.
fn sound(
    profile: &coppa_codec::ofdm::CoppaProfile,
    level: u8,
    papr_target_db: f32,
    snr: f32,
    base: u64,
) -> Vec<f32> {
    let modem = CoppaModem::new(profile.clone(), 1);
    let symbols = build_symbols(level);
    let header = make_header(level, (symbols.len() / 8).max(1) as u16);
    let sig = modem.modulate_mapped(&header, &symbols, papr_target_db);

    let mut out = Vec::with_capacity(TRIALS);
    for t in 0..TRIALS {
        let seed = base
            .wrapping_add(t as u64)
            .wrapping_add((level as u64).wrapping_mul(0x1000_0000));
        let faded = coppa_channel::awgn_seeded(&sig, snr, seed ^ 0x5555);
        if let Some((h, _eq, nv)) = modem.demodulate_frame(&faded) {
            if h.speed_level == level {
                out.push(channel_capacity(&nv));
            }
        }
    }
    out
}

fn main() {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| {
            u64::from_str_radix(s.trim_start_matches("0x"), 16)
                .ok()
                .or_else(|| s.parse().ok())
        })
        .unwrap_or(0x000C_A11B);
    let profile = profile_by_name("robust").unwrap();
    let snrs = [6.0f32, 12.0, 18.0, 24.0, 30.0];

    let conditions: [PaprCondition; 3] = [
        ("natural", |level: u8| {
            SPEED_LEVELS
                .iter()
                .find(|s| s.level == level)
                .unwrap()
                .papr_target_db
        }),
        ("uniform_loose", |_level: u8| UNIFORM_LOOSE_DB),
        ("uniform_harsh", |_level: u8| UNIFORM_HARSH_DB),
    ];

    eprintln!("papr_clip_level_bias seed=0x{seed:X}");

    // cells[cond_idx][snr_idx][level_idx] = (mean_capacity, std_capacity)
    let mut cells: Vec<Vec<Vec<(f32, f32)>>> = Vec::new();

    for (cname, papr_fn) in &conditions {
        let mut cond_rows = Vec::new();
        for &snr in &snrs {
            let mut row = Vec::new();
            for &level in &LEVELS {
                let papr_target = papr_fn(level);
                let caps = sound(&profile, level, papr_target, snr, seed);
                let (mean_c, std_c) = mean_std(&caps);
                println!(
                    "DATA {cname} {snr:.0} {level} {mean_c:.3} {std_c:.3} (n={}/{TRIALS}, papr={papr_target:.1}dB)",
                    caps.len()
                );
                row.push((mean_c, std_c));
            }
            cond_rows.push(row);
        }
        cells.push(cond_rows);
    }

    println!();
    println!("SUMMARY (per condition, averaged across SNR grid):");
    for (ci, (cname, _)) in conditions.iter().enumerate() {
        let mut gaps = Vec::new();
        let mut se_bounds = Vec::new();
        for row in &cells[ci] {
            let (lo_mean, lo_std) = row[0];
            let (hi_mean, hi_std) = row[row.len() - 1];
            gaps.push(hi_mean - lo_mean);
            let se_lo = lo_std / (TRIALS as f32).sqrt();
            let se_hi = hi_std / (TRIALS as f32).sqrt();
            se_bounds.push(2.0 * (se_lo + se_hi));
        }
        let (mean_gap, _) = mean_std(&gaps);
        let (mean_se_bound, _) = mean_std(&se_bounds);
        let verdict = if mean_gap.abs() > mean_se_bound {
            "LIKELY REAL TREND"
        } else {
            "within noise"
        };
        println!(
            "  {cname}: level1->level10 mean capacity gap = {mean_gap:+.3} bits/s/Hz \
             (2*SE bound = {mean_se_bound:.3}) -> {verdict}"
        );
    }

    println!();
    println!("INTERPRETATION: if `uniform_loose`/`uniform_harsh` gaps are both");
    println!("substantially smaller than `natural`'s, the differential per-level");
    println!("papr_target_db is a real contributor to the level-dependence PR #51");
    println!("found in AWGN. If they remain comparable to `natural`, that falsifies");
    println!("PAPR clipping as the mechanism.");
}
