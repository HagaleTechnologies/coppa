//! Phase 2 Task 4 bench gate: NR BG2 mother code + rate matching + layered
//! decoder vs. the pre-Task-4 per-rate 802.11 QC-LDPC codec.
//!
//! Two sweeps, per the Task 4 brief:
//!
//! 1. **Isolated FEC-layer AWGN sweep** (bypass OFDM entirely, same
//!    masking-free methodology `task3_fec_isolated_gate` established): the
//!    old and new codecs are compared at the FEC layer directly -- per-level
//!    constellation mapper, AWGN on the mapped symbols, soft demap, decode.
//!    This is the authoritative measurement for the acceptance gate
//!    ("level-2 AWGN FER<=10% threshold improves >= 1.2 dB, no level
//!    regresses > 0.3 dB"): Task 3's report already established that a
//!    small/heavily-padded payload's full-system FER curve is dominated by
//!    OFDM sync, not LDPC, at every SNR where LDPC could plausibly fail --
//!    so this sweep deliberately uses a **near-full-capacity** payload (not
//!    a small padded one) to exercise the code at close to its designed
//!    rate, and skips OFDM/sync/interleaving entirely so the LDPC's own
//!    margin is never masked by anything else.
//! 2. **Full end-to-end OFDM sweep, Watterson-Poor + AWGN** ("poor" channel
//!    condition): both codecs driven through the *same* `CoppaModem`
//!    OFDM modulate/demodulate pipeline (old codec manually assembled
//!    exactly as `CoppaTransceiver` used to; new codec via the actual
//!    current `CoppaTransceiver`), same channel realizations. This can in
//!    principle be masked by sync/header recovery the same way Task 3
//!    found for AWGN -- reported honestly either way, not papered over.
//!
//! Additionally times decode CPU/frame (old vs new) and records the new
//! layered decoder's early-exit iteration statistics.
//!
//! Run: `cargo run -p coppa-bench --release --example task4_bg2_ldpc_gate`

use coppa_channel::watterson::{watterson, WattersonPreset};
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::fec::ldpc::rate_match::{rate_dematch, rate_match};
use coppa_protocol::fec::ldpc::{pin_known_pad, CodeRate, LdpcCodec, NrLdpc};
use coppa_protocol::fec::scrambler::scramble;
use coppa_protocol::modem::speed_levels::{
    k_used_for_level, max_payload_for_level, speed_level_components,
};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::time::Instant;

const CODED_LEN: usize = 1944;
const LEVELS: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 9, 10];

fn old_code_rate(level: u8) -> CodeRate {
    match level {
        1 => CodeRate::Rate1_4,
        2 => CodeRate::Rate1_2,
        3 => CodeRate::Rate1_2,
        4 => CodeRate::Rate3_4,
        5 => CodeRate::Rate2_3,
        6 => CodeRate::Rate1_2,
        7 => CodeRate::Rate3_4,
        9 => CodeRate::Rate2_3,
        10 => CodeRate::Rate7_8,
        _ => panic!("no old code rate for level {level}"),
    }
}

/// Deterministic pseudo-random payload bits (ground truth, pre-scramble):
/// `payload_bits` random bits followed by zero padding out to `total_len`.
fn ground_truth_info(payload_bits: usize, total_len: usize, seed: u64) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut v: Vec<u8> = (0..payload_bits)
        .map(|_| rng.random_range(0..2u8))
        .collect();
    v.resize(total_len, 0u8);
    v
}

fn awgn_symbols(
    symbols: &[num_complex::Complex32],
    snr_db: f32,
    seed: u64,
) -> Vec<num_complex::Complex32> {
    let signal_power: f32 =
        symbols.iter().map(|s| s.norm_sqr()).sum::<f32>() / symbols.len() as f32;
    let noise_power = signal_power / 10f32.powf(snr_db / 10.0);
    let noise_std = (noise_power / 2.0).sqrt();
    let mut rng = StdRng::seed_from_u64(seed);
    symbols
        .iter()
        .map(|&s| {
            let u1: f32 = rng.random::<f32>().max(1e-10);
            let u2: f32 = rng.random();
            let n_re =
                noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
            let u3: f32 = rng.random::<f32>().max(1e-10);
            let u4: f32 = rng.random();
            let n_im =
                noise_std * (-2.0 * u3.ln()).sqrt() * (2.0 * std::f32::consts::PI * u4).cos();
            s + num_complex::Complex32::new(n_re, n_im)
        })
        .collect()
}

// =========================================================================
// 1. Isolated FEC-layer AWGN sweep
// =========================================================================

fn trial_old(level: u8, payload_bits: usize, snr_db: f32, seed: u64) -> bool {
    let codec = LdpcCodec::new(old_code_rate(level));
    let info_bits = codec.code().info_bits();
    let (mapper, _) = speed_level_components(level).unwrap();

    let mut info = ground_truth_info(payload_bits, info_bits, seed);
    scramble(&mut info);
    let coded = codec.encode(&info);

    let symbols = mapper.map_bits(&coded);
    let noisy = awgn_symbols(&symbols, snr_db, seed ^ 0xA11CE);
    let nv = 10f32.powf(-snr_db / 10.0);
    let mut llrs = Vec::with_capacity(CODED_LEN);
    for &s in &noisy {
        llrs.extend(mapper.demap_soft(s, nv));
    }
    llrs.truncate(CODED_LEN);

    let (mut decoded, converged) = codec.decode_checked(&llrs);
    if !converged {
        return false;
    }
    scramble(&mut decoded);
    let truth = ground_truth_info(payload_bits, info_bits, seed);
    decoded[..payload_bits] == truth[..payload_bits]
}

fn trial_new(level: u8, payload_bits: usize, snr_db: f32, seed: u64, ldpc: &NrLdpc) -> bool {
    let k_used = k_used_for_level(level).unwrap();
    let (mapper, _) = speed_level_components(level).unwrap();

    let mut info = ground_truth_info(payload_bits, NrLdpc::INFO_LEN, seed);
    scramble(&mut info);
    let mother = ldpc.encode(&info);
    let matched = rate_match(&mother, k_used, CODED_LEN, 0);

    let symbols = mapper.map_bits(&matched);
    let noisy = awgn_symbols(&symbols, snr_db, seed ^ 0xA11CE);
    let nv = 10f32.powf(-snr_db / 10.0);
    let mut llrs = Vec::with_capacity(CODED_LEN);
    for &s in &noisy {
        llrs.extend(mapper.demap_soft(s, nv));
    }
    llrs.truncate(CODED_LEN);

    let mut dematched = rate_dematch(&llrs, k_used, CODED_LEN, 0, NrLdpc::MOTHER_LEN);
    pin_known_pad(&mut dematched, payload_bits, k_used, 64.0);
    let (_, mut decoded, converged) = ldpc.decode_soft(&dematched);
    if !converged {
        return false;
    }
    scramble(&mut decoded);
    let truth = ground_truth_info(payload_bits, NrLdpc::INFO_LEN, seed);
    decoded[..payload_bits] == truth[..payload_bits]
}

fn isolated_fer_at(level: u8, new_codec: bool, snr_db: f32, trials: usize, ldpc: &NrLdpc) -> f64 {
    let k_used = k_used_for_level(level).unwrap();
    let old_info_bits = LdpcCodec::new(old_code_rate(level)).code().info_bits();
    // Near-full-capacity payload (a byte short of full width) so the code
    // operates close to its designed rate, not artificially overprotected
    // by heavy known-pad pinning (see module docs).
    let payload_bits = if new_codec { k_used } else { old_info_bits } - 8;

    let mut fails = 0usize;
    for t in 0..trials {
        let seed = 0x7A5C_0000u64
            .wrapping_add((level as u64) << 40)
            .wrapping_add(((snr_db * 10.0) as i64 as u64) << 20)
            .wrapping_add(t as u64);
        let ok = if new_codec {
            trial_new(level, payload_bits, snr_db, seed, ldpc)
        } else {
            trial_old(level, payload_bits, snr_db, seed)
        };
        if !ok {
            fails += 1;
        }
    }
    fails as f64 / trials as f64
}

/// Locate the lowest SNR (dB, granularity `step`) with FER<=10% between `lo`
/// and `hi`. Coarse pass at `trials/4` to bracket the crossing quickly, then
/// a full-`trials` re-measurement right at the bracketed point.
fn find_fer10_threshold(
    level: u8,
    new_codec: bool,
    lo: f32,
    hi: f32,
    step: f32,
    trials: usize,
    ldpc: &NrLdpc,
) -> Option<(f32, f64)> {
    let coarse_trials = (trials / 4).max(50);
    let mut snr = lo;
    while snr <= hi + 1e-6 {
        let fer = isolated_fer_at(level, new_codec, snr, coarse_trials, ldpc);
        if fer <= 0.10 {
            let fine_fer = isolated_fer_at(level, new_codec, snr, trials, ldpc);
            return Some((snr, fine_fer));
        }
        snr += step;
    }
    None
}

// =========================================================================
// 2. Full end-to-end OFDM sweep (Watterson-Poor + AWGN)
// =========================================================================

fn profile_for(level: u8) -> CoppaProfile {
    if level >= 5 {
        CoppaProfile::vhf_wide()
    } else {
        CoppaProfile::hf_standard()
    }
}

fn make_header(level: u8, payload_len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len,
        codewords: 1,
    }
}

/// Manually-assembled OLD-codec TX+RX through the real OFDM pipeline --
/// exactly what `CoppaTransceiver` did before Task 4 (see git history /
/// ADR-005). Returns whether the payload round-tripped exactly.
fn old_ofdm_trial(level: u8, payload: &[u8], snr_db: f32, seed: u64) -> bool {
    let profile = profile_for(level);
    let modem = CoppaModem::new(profile.clone(), 1);
    let (mapper, code_rate) = speed_level_components(level).unwrap();
    let codec = LdpcCodec::new(code_rate);
    let interleaver = BlockInterleaver::new(CODED_LEN, profile.data_carriers);
    let sl = coppa_codec::ofdm::coppa_modem::SPEED_LEVELS
        .iter()
        .find(|s| s.level == level)
        .unwrap();

    let info_bits = codec.code().info_bits();
    let mut info = Vec::with_capacity(info_bits);
    for &byte in payload {
        for shift in (0..8).rev() {
            info.push((byte >> shift) & 1);
        }
    }
    info.resize(info_bits, 0u8);
    scramble(&mut info);
    let coded = codec.encode(&info);
    let interleaved = interleaver.interleave(&coded);
    let symbols = mapper.map_bits(&interleaved);
    let header = make_header(level, payload.len() as u16);
    let clean = modem.modulate_mapped(&header, &symbols, sl.papr_target_db);

    let faded = watterson(&clean, 48_000.0, &WattersonPreset::Poor.config(), seed);
    let noisy = coppa_channel::awgn_seeded(&faded, snr_db, seed ^ 0x5A5A);

    let Some((rx_header, eq_symbols, noise_vars)) = modem.demodulate_frame(&noisy) else {
        return false;
    };
    if rx_header.speed_level != level {
        return false;
    }
    let bps = mapper.bits_per_symbol();
    let symbols_needed = CODED_LEN.div_ceil(bps);
    let fallback_nv = 1.0f32;
    let mut llrs = Vec::with_capacity(CODED_LEN);
    for (i, &sym) in eq_symbols.iter().take(symbols_needed).enumerate() {
        let nv = match noise_vars.get(i) {
            Some(&v) if v > 1e-6 => v,
            _ => fallback_nv,
        };
        llrs.extend(mapper.demap_soft(sym, nv));
    }
    llrs.truncate(CODED_LEN);
    llrs.resize(CODED_LEN, 0.0);
    for l in &mut llrs {
        *l = l.clamp(-20.0, 20.0);
    }
    let deinterleaved = interleaver.deinterleave(&llrs);
    let (mut decoded, converged) = codec.decode_checked(&deinterleaved);
    if !converged {
        return false;
    }
    scramble(&mut decoded);
    let mut out = Vec::with_capacity(payload.len());
    for chunk in decoded.chunks(8) {
        if chunk.len() == 8 && out.len() < payload.len() {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - i);
            }
            out.push(byte);
        }
    }
    out == payload
}

fn new_ofdm_trial(
    tx: &CoppaTransceiver,
    level: u8,
    payload: &[u8],
    snr_db: f32,
    seed: u64,
) -> bool {
    let header = make_header(level, payload.len() as u16);
    let clean = tx
        .transmit(&header, payload)
        .expect("payload within this level's capacity");
    let faded = watterson(&clean, 48_000.0, &WattersonPreset::Poor.config(), seed);
    let noisy = coppa_channel::awgn_seeded(&faded, snr_db, seed ^ 0x5A5A);
    match tx.receive(&noisy) {
        Ok((rx_header, rx_payload, _rec_level)) => {
            rx_header.speed_level == level && rx_payload[..payload.len()] == payload[..]
        }
        Err(_) => false,
    }
}

fn ofdm_fer_at(level: u8, new_codec: bool, snr_db: f32, trials: usize, payload: &[u8]) -> f64 {
    let tx_new = if new_codec {
        Some(CoppaTransceiver::new(profile_for(level), 1))
    } else {
        None
    };
    let mut fails = 0usize;
    for t in 0..trials {
        let seed = 0x0FAD_E000u64
            .wrapping_add((level as u64) << 40)
            .wrapping_add(((snr_db * 10.0) as i64 as u64) << 20)
            .wrapping_add(t as u64);
        let ok = if new_codec {
            new_ofdm_trial(tx_new.as_ref().unwrap(), level, payload, snr_db, seed)
        } else {
            old_ofdm_trial(level, payload, snr_db, seed)
        };
        if !ok {
            fails += 1;
        }
    }
    fails as f64 / trials as f64
}

// =========================================================================
// 3. Decode CPU/frame + early-exit iteration stats (new decoder only)
// =========================================================================

fn timing_and_iterations(level: u8, ldpc: &NrLdpc, snr_db: f32, trials: usize) -> (f64, f64, f64) {
    let k_used = k_used_for_level(level).unwrap();
    let old_codec = LdpcCodec::new(old_code_rate(level));
    let old_info_bits = old_codec.code().info_bits();

    // Pre-build one representative noisy LLR set per codec at this SNR/level
    // (timing measures decode() cost alone, not encode/channel simulation).
    let seed = 0x51DE_0000u64.wrapping_add(level as u64);

    let old_llrs = {
        let mut info = vec![0u8; old_info_bits];
        scramble(&mut info);
        let coded = old_codec.encode(&info);
        let (mapper, _) = speed_level_components(level).unwrap();
        let symbols = mapper.map_bits(&coded);
        let noisy = awgn_symbols(&symbols, snr_db, seed);
        let nv = 10f32.powf(-snr_db / 10.0);
        let mut llrs = Vec::with_capacity(CODED_LEN);
        for &s in &noisy {
            llrs.extend(mapper.demap_soft(s, nv));
        }
        llrs.truncate(CODED_LEN);
        llrs
    };

    let new_dematched = {
        let mut info = vec![0u8; NrLdpc::INFO_LEN];
        scramble(&mut info);
        let mother = ldpc.encode(&info);
        let matched = rate_match(&mother, k_used, CODED_LEN, 0);
        let (mapper, _) = speed_level_components(level).unwrap();
        let symbols = mapper.map_bits(&matched);
        let noisy = awgn_symbols(&symbols, snr_db, seed);
        let nv = 10f32.powf(-snr_db / 10.0);
        let mut llrs = Vec::with_capacity(CODED_LEN);
        for &s in &noisy {
            llrs.extend(mapper.demap_soft(s, nv));
        }
        llrs.truncate(CODED_LEN);
        let mut dematched = rate_dematch(&llrs, k_used, CODED_LEN, 0, NrLdpc::MOTHER_LEN);
        pin_known_pad(&mut dematched, 0, k_used, 64.0);
        dematched
    };

    let t0 = Instant::now();
    for _ in 0..trials {
        std::hint::black_box(old_codec.decode_checked(std::hint::black_box(&old_llrs)));
    }
    let old_us_per_frame = t0.elapsed().as_secs_f64() * 1e6 / trials as f64;

    let mut total_iters = 0u64;
    let t1 = Instant::now();
    for _ in 0..trials {
        let (_, _, _, iters) = ldpc.decode_soft_stats(std::hint::black_box(&new_dematched));
        total_iters += iters as u64;
    }
    let new_us_per_frame = t1.elapsed().as_secs_f64() * 1e6 / trials as f64;
    let avg_iters = total_iters as f64 / trials as f64;

    (old_us_per_frame, new_us_per_frame, avg_iters)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let trials: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(400usize);
    let ofdm_trials: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100usize);

    eprintln!("=== Task 4 BG2 LDPC gate: isolated_trials={trials} ofdm_trials={ofdm_trials} ===\n");

    let ldpc = NrLdpc::new();

    println!("## 1. Isolated FEC-layer AWGN sweep (near-full-capacity payload)\n");
    println!("| level | old FER<=10% (dB) | new FER<=10% (dB) | delta (dB, +=better) |");
    println!("|---|---|---|---|");
    let mut level2_delta = None;
    let mut regressions = Vec::new();
    let mut thresholds: std::collections::HashMap<u8, (f32, f32)> =
        std::collections::HashMap::new();
    for &level in &LEVELS {
        let old_t = find_fer10_threshold(level, false, -12.0, 30.0, 0.5, trials, &ldpc);
        let new_t = find_fer10_threshold(level, true, -12.0, 30.0, 0.5, trials, &ldpc);
        match (old_t, new_t) {
            (Some((old_snr, old_fer)), Some((new_snr, new_fer))) => {
                let delta = old_snr - new_snr;
                println!(
                    "| {level} | {old_snr:.1} (fer={old_fer:.3}) | {new_snr:.1} (fer={new_fer:.3}) | {delta:+.2} |"
                );
                thresholds.insert(level, (old_snr, new_snr));
                if level == 2 {
                    level2_delta = Some(delta);
                }
                if delta < -0.3 {
                    regressions.push((level, delta));
                }
            }
            _ => println!("| {level} | (no threshold found in range) | | |"),
        }
    }
    println!();
    match level2_delta {
        Some(d) if d >= 1.2 => {
            println!("ACCEPTANCE (level-2 AWGN): PASS, delta={d:+.2} dB (>= 1.2 dB required)")
        }
        Some(d) => {
            println!("ACCEPTANCE (level-2 AWGN): FAIL, delta={d:+.2} dB (< 1.2 dB required)")
        }
        None => println!("ACCEPTANCE (level-2 AWGN): INCONCLUSIVE (no threshold found)"),
    }
    if regressions.is_empty() {
        println!("ACCEPTANCE (no level regresses > 0.3 dB): PASS");
    } else {
        println!("ACCEPTANCE (no level regresses > 0.3 dB): FAIL -- {regressions:?}");
    }
    println!();

    println!("## 2. Full OFDM sweep, Watterson-Poor + AWGN (near-full-capacity payload)\n");
    println!("| level | SNR (dB) | old FER | new FER |");
    println!("|---|---|---|---|");
    for &level in &LEVELS {
        let old_info_bits = LdpcCodec::new(old_code_rate(level)).code().info_bits();
        // Old codec's own historic 1-byte margin below its raw info-bit capacity.
        let old_payload_bytes = (old_info_bits - 8) / 8;
        // New codec's real capacity via `CoppaTransceiver::transmit`'s actual
        // accessor (Phase 3 Task 1: reserves 4 bytes for the CRC-32 trailer
        // `transmit` appends -- the old `k_used/8 - 8bits` margin here predates
        // that and isn't wide enough, which made `new_ofdm_trial`'s `transmit`
        // call panic with `PayloadTooLarge`).
        let new_payload_bytes = max_payload_for_level(level).unwrap();
        let payload_bytes = old_payload_bytes.min(new_payload_bytes);
        let payload: Vec<u8> = (0..payload_bytes).map(|i| (i * 37 + 5) as u8).collect();

        // Informed base SNR per level (coarse, from the existing
        // `tests/phase_c_loopback.rs::required_snr` heuristic) + a fading
        // margin, then a handful of points around it.
        let base = match level {
            1 => 8.0,
            2 => 10.0,
            3 => 12.0,
            4 => 16.0,
            5 => 18.0,
            6 => 18.0,
            7 => 22.0,
            9 => 26.0,
            10 => 30.0,
            _ => 20.0,
        };
        for &delta in &[-4.0f32, 0.0, 4.0, 8.0] {
            let snr = base + delta;
            let old_fer = ofdm_fer_at(level, false, snr, ofdm_trials, &payload);
            let new_fer = ofdm_fer_at(level, true, snr, ofdm_trials, &payload);
            println!("| {level} | {snr:.1} | {old_fer:.3} | {new_fer:.3} |");
        }
    }
    println!();

    println!("## 3. Decode CPU/frame + early-exit iteration stats\n");
    println!(
        "Each level's timing SNR = max(old_threshold, new_threshold) + 6 dB from section 1 -- a"
    );
    println!("representative \"normal successful decode\" operating point per level, NOT a single");
    println!(
        "fixed SNR for every level (a fixed low SNR would leave high-rate levels always hitting"
    );
    println!("the iteration cap, which is not representative of real per-frame cost).\n");
    println!("| level | timing SNR (dB) | old us/frame | new us/frame | ratio (new/old) | new avg iterations |");
    println!("|---|---|---|---|---|---|");
    for &level in &LEVELS {
        let timing_snr = thresholds
            .get(&level)
            .map(|&(old_t, new_t)| old_t.max(new_t) + 6.0)
            .unwrap_or(20.0);
        let (old_us, new_us, avg_iters) = timing_and_iterations(level, &ldpc, timing_snr, 2000);
        let ratio = new_us / old_us;
        println!("| {level} | {timing_snr:.1} | {old_us:.2} | {new_us:.2} | {ratio:.2}x | {avg_iters:.2} |");
    }
}
