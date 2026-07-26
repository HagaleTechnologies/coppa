//! Golden-vector generator (Task 8, decision 9c): writes 20 reference WAV
//! files + a manifest under `testdata/golden/`, used by
//! `crates/coppa-protocol/tests/golden_vectors.rs` as a frozen decode
//! regression corpus -- a future PHY/FEC change that silently breaks decoding
//! of these exact frames is now a visible, committed-test failure, not
//! something only a full bench sweep would notice.
//!
//! Run from the workspace root (writes to `testdata/golden/`, a relative path):
//! `cargo run -p coppa-bench --release --example golden_vectors_gen`
//!
//! This is a generator tool, not a "bench" -- it lives in `examples/` alongside
//! the milstd/session benches (Task 8's other two deliverables) for the same
//! reason they do (see `milstd.rs`'s module doc for the CLI-structure
//! rationale), but its job is to produce committed artifacts, not print a
//! measurement report.
//!
//! ## Combinations
//!
//! levels {1, 2, 5, 6, 9} x channels {clean, awgn@12dB, poor@25dB, ssb+cfo} = 20
//! WAVs, each 48 kHz / 16-bit PCM mono (`hound`, matching
//! `coppa_audio::file_backend`'s read/write conventions but 16-bit int rather
//! than that module's `WavSink`'s 32-bit float, per this task's brief).
//!
//! ## Profile override for the `poor25` and `ssbcfo` conditions
//!
//! Every level uses `hf_standard` (not the per-level default `vhf_wide`
//! routing) for the `poor25` and `ssbcfo` channel conditions specifically:
//! `vhf_wide`'s 60-sample (1.25 ms) cyclic prefix is shorter than Watterson
//! Poor's 2.0 ms delay spread (causing total decode failure regardless of
//! SNR, confirmed by a direct A/B while building this task's `milstd` bench),
//! and `ssb_filter`'s 300-2700 Hz passband is narrower than `vhf_wide`'s own
//! ~350-5900 Hz active band (an SSB rig audio passband is an HF-specific
//! impairment, not applicable to a VHF-routed profile at all). `clean`/`awgn12`
//! have no multipath or out-of-band filtering, so they keep each level's
//! normal profile routing.
//!
//! ## Level 9's exception -- CORRECTED 2026-07-26 (was a bench bug, not a codec limitation)
//!
//! Level 9 (64QAM 2/3) previously needed non-literal operating points here,
//! and `L9_poor25` was committed with `expected_decode_ok = false` as a
//! documented "structurally undecodable at any SNR" case. **Both were wrong.**
//! Root cause: this generator's seed-search loop reused one `CoppaTransceiver`
//! (and hardcoded `seq_num: 0`) across up to `MAX_SEED_ATTEMPTS` attempts.
//! `CoppaTransceiver`'s IR-HARQ receive-side LLR accumulator only evicts on a
//! successful decode, so the first attempt that failed to converge (routine
//! at a marginal operating point -- that's what a seed search is for) left
//! its buffer un-evicted, and every subsequent attempt at that seq inherited
//! the contamination -- cascading into "no seed in 500 attempts decoded" or
//! "only seed N out of hundreds happened to work before the buffer was
//! poisoned." This is the exact same bug found and fixed in
//! `crates/coppa-bench/src/runner.rs` (see
//! `ascending_sweep_low_snr_failure_does_not_poison_later_high_snr_trials`
//! there); this generator's search loop had the identical vulnerable shape
//! and just hadn't been checked.
//!
//! Fixed by evicting the HARQ buffer unconditionally after every attempt
//! (regardless of outcome), then re-measured level 9 directly (100-300
//! trials/point, fresh transceiver + eviction per trial, bypassing this
//! generator's seed-search framing entirely):
//!
//! - **AWGN** (`vhf_wide`, this level's default profile): a completely
//!   ordinary waterfall -- 100% FER at 12/15 dB (genuinely below threshold),
//!   6% at 18 dB, then a clean 0% from 21-27 dB, 1% at 30 dB. Real threshold
//!   is ~18-21 dB, not "30 dB and still mostly failing." `LEVEL9_AWGN_SNR_DB`
//!   is corrected to 24 dB (comfortably clear, matching the margin other
//!   levels' reference SNRs use).
//! - **ssb+cfo** (`hf_standard` forced, 15 Hz CFO): even cleaner -- 0% FER
//!   at every tested point from 18-33 dB. `LEVEL9_SSBCFO_SNR_DB` corrected
//!   to 24 dB.
//! - **Watterson Poor** (`hf_standard` forced, matching this combination's
//!   profile override): genuinely bad, but NOT zero -- 86-91% FER at 25 dB
//!   (100-300 trials), flat out to 54 dB. A ~9-14% real success rate is
//!   easily enough for a 500-attempt seed search to find a decodable frame
//!   (verified: it does). This IS a real, severe channel-estimation-class
//!   gap for 64QAM under deep HF fading -- consistent with CLAUDE.md's
//!   "Phase 2 channel estimation" and "Turbo re-estimation... concentrated
//!   on low-order modulation" limitations -- just not the literal "0% at any
//!   SNR" this doc previously claimed. `L9_poor25` is now generated like
//!   every other combination (searched, decoded, `expected_decode_ok = true`)
//!   rather than hand-classified as an unconditional failure.
//!
//! Root-causing *why* level 9 specifically (vs. levels 1/2/5/6 here) is worse
//! under Watterson Poor is still out of scope for this generator -- that part
//! of the original diagnosis stands, just corrected from "impossible" to
//! "real but not universal."
//!
//! ## Seed selection
//!
//! This generator searches a small, deterministic sequence of payload seeds
//! (see `MAX_SEED_ATTEMPTS`) for one that this commit's codec actually
//! decodes correctly at that operating point, for every combination
//! (including `L9_poor25` as of the correction above) -- this is meant to be
//! a KNOWN-GOOD regression corpus (a future regression is "this exact frame
//! no longer decodes", not "we rolled dice and got unlucky at generation
//! time"). The manifest records which seed (and how many attempts) each
//! entry needed. A combination that exhausts `MAX_SEED_ATTEMPTS` without a
//! clean decode is skipped with a warning rather than silently committed --
//! see the bug this replaced above for why that skip condition itself needs
//! a HARQ-buffer-eviction guarantee to be trustworthy.

use std::path::PathBuf;

use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const LEVELS: &[u8] = &[1, 2, 5, 6, 9];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    Clean,
    Awgn12,
    Poor25,
    SsbCfo,
}

impl Channel {
    fn id(self) -> &'static str {
        match self {
            Channel::Clean => "clean",
            Channel::Awgn12 => "awgn12",
            Channel::Poor25 => "poor25",
            Channel::SsbCfo => "ssbcfo",
        }
    }
}

const CHANNELS: &[Channel] = &[
    Channel::Clean,
    Channel::Awgn12,
    Channel::Poor25,
    Channel::SsbCfo,
];

/// Default AWGN SNR (dB, 3 kHz-referenced) for the `awgn12` combination.
const DEFAULT_AWGN_SNR_DB: f32 = 12.0;
/// Default AWGN SNR (dB, 3 kHz-referenced) for the `poor25` combination
/// (applied on top of Watterson Poor fading).
const DEFAULT_POOR_SNR_DB: f32 = 25.0;
/// Default AWGN SNR (dB, 3 kHz-referenced) for the `ssbcfo` combination.
const DEFAULT_SSBCFO_SNR_DB: f32 = 20.0;
/// CFO applied for the `ssbcfo` combination (Hz) -- well within the documented
/// +-50 Hz two-stage-acquisition tolerance (CLAUDE.md Known Limitations).
const SSB_CFO_HZ: f32 = 15.0;

/// Level 9's AWGN/ssb+cfo operating points. Corrected 2026-07-26 (see module
/// doc's "Level 9's exception" section): the literal 12/20 dB grid values
/// are still too low for 64QAM 2/3, but the level's real threshold is an
/// ordinary ~18-21 dB AWGN / ~18 dB ssb+cfo -- NOT the ~30 dB "late, steep,
/// seed-dependent waterfall" this generator previously (incorrectly)
/// characterized. These values are comfortably above that real threshold
/// (verified 0% FER over 100 trials at both), not marginal seed-search
/// anchors.
const LEVEL9_AWGN_SNR_DB: f32 = 24.0;
const LEVEL9_SSBCFO_SNR_DB: f32 = 24.0;

/// Deterministic payload seeds to try per combination before giving up.
const MAX_SEED_ATTEMPTS: u64 = 500;

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

/// Profile to transmit/receive `level` with, for `channel`. See module doc:
/// `poor25` and `ssbcfo` force `hf_standard` for every level.
fn profile_for(level: u8, channel: Channel) -> CoppaProfile {
    match channel {
        Channel::Poor25 | Channel::SsbCfo => CoppaProfile::hf_standard(),
        _ => select_profile(level),
    }
}

/// AWGN SNR (dB) to use for `level`/`channel`'s AWGN component. See module
/// doc: level 9 needs a documented, verified exception for `awgn12`/`ssbcfo`.
fn awgn_snr_for(level: u8, channel: Channel) -> f32 {
    match (level, channel) {
        (9, Channel::Awgn12) => LEVEL9_AWGN_SNR_DB,
        (9, Channel::SsbCfo) => LEVEL9_SSBCFO_SNR_DB,
        (_, Channel::Awgn12) => DEFAULT_AWGN_SNR_DB,
        (_, Channel::Poor25) => DEFAULT_POOR_SNR_DB,
        (_, Channel::SsbCfo) => DEFAULT_SSBCFO_SNR_DB,
        (_, Channel::Clean) => f32::INFINITY,
    }
}

/// Apply `channel` to `clean` (the TX signal) at `snr_db` (ignored for
/// `Channel::Clean`), returning the RX-side signal.
fn apply_channel(channel: Channel, clean: &[f32], snr_db: f32, seed: u64) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    match channel {
        Channel::Clean => clean.to_vec(),
        Channel::Awgn12 => {
            let p_clean = coppa_channel::mean_power(clean);
            coppa_channel::awgn_ref_seeded(clean, snr_db, p_clean, sr, seed ^ 0x5555)
        }
        Channel::Poor25 => {
            let p_clean = coppa_channel::mean_power(clean);
            let faded = coppa_channel::watterson::watterson_preset(
                clean,
                sr,
                WattersonPreset::Poor,
                seed ^ 0x3333,
            );
            coppa_channel::awgn_ref_seeded(&faded, snr_db, p_clean, sr, seed ^ 0x5555)
        }
        Channel::SsbCfo => {
            let filtered = coppa_channel::ssb_filter(clean, sr);
            let p_clean = coppa_channel::mean_power(&filtered);
            let noisy =
                coppa_channel::awgn_ref_seeded(&filtered, snr_db, p_clean, sr, seed ^ 0x5555);
            coppa_channel::frequency_shift(&noisy, SSB_CFO_HZ, sr)
        }
    }
}

fn write_wav_i16(path: &std::path::Path, samples: &[f32], sample_rate: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create golden WAV");
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let v = (clamped * i16::MAX as f32).round() as i16;
        writer.write_sample(v).expect("write golden WAV sample");
    }
    writer.finalize().expect("finalize golden WAV");
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct GeneratedVector {
    id: String,
    level: u8,
    channel: &'static str,
    seed: u64,
    attempts: u64,
    payload_hex: String,
    wav_file: String,
    snr_db: Option<f32>,
    cfo_hz: Option<f32>,
    expected_decode_ok: bool,
}

fn main() {
    let out_dir = PathBuf::from("testdata/golden");
    std::fs::create_dir_all(&out_dir).expect(
        "create testdata/golden (run this from the workspace root: \
         `cargo run -p coppa-bench --release --example golden_vectors_gen`)",
    );

    let mut generated = Vec::new();

    for &level in LEVELS {
        let mode = mode_for_level(level).expect("valid level");
        let payload_bytes = mode.payload_bytes();

        for &channel in CHANNELS {
            let profile = profile_for(level, channel);
            let tx = CoppaTransceiver::new(profile, 1);
            let id = format!("L{level}_{}", channel.id());
            let snr_db = awgn_snr_for(level, channel);

            // Deterministic per-(level,channel) base seed (FNV-ish mix).
            let base_seed = {
                let mut h = 0x9E3779B97F4A7C15u64;
                h ^= level as u64;
                h = h.wrapping_mul(0x100_0000_01B3);
                h ^= channel
                    .id()
                    .bytes()
                    .fold(0u64, |a, b| a.wrapping_add(b as u64));
                h
            };

            let mut found = None;
            for attempt in 0..MAX_SEED_ATTEMPTS {
                let seed = base_seed.wrapping_add(attempt);
                let mut rng = StdRng::seed_from_u64(seed);
                let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
                let header = make_header(level, payload_bytes as u16);
                let clean = tx
                    .transmit(&header, &payload)
                    .expect("payload within this level's capacity");
                let rx_signal = apply_channel(channel, &clean, snr_db, seed);

                let decoded_ok = if let Ok((_h, bytes, _lvl)) = tx.receive(&rx_signal) {
                    bytes.len() >= payload.len() && bytes[..payload.len()] == payload[..]
                } else {
                    false
                };
                // Every attempt here is an independent random-payload draw sharing
                // seq_num 0 on one `tx` reused across `MAX_SEED_ATTEMPTS` attempts --
                // not a real retransmission. Evict unconditionally so an attempt
                // that fails to converge (expected during a search at a marginal
                // operating point) can't corrupt the next attempt's IR-HARQ
                // accumulator. See coppa-bench's
                // `runner.rs::ascending_sweep_low_snr_failure_does_not_poison_later_high_snr_trials`
                // for the bug this mirrors.
                tx.harq_evict(0);
                if decoded_ok {
                    found = Some((seed, attempt, payload, rx_signal));
                    break;
                }
            }

            let Some((seed, attempts, payload, rx_signal)) = found else {
                eprintln!(
                    "WARNING: {id}: no seed in {MAX_SEED_ATTEMPTS} attempts decoded cleanly -- SKIPPED"
                );
                continue;
            };

            let wav_name = format!("{id}.wav");
            write_wav_i16(&out_dir.join(&wav_name), &rx_signal, SAMPLE_RATE);

            println!(
                "{id}: seed=0x{seed:016X} attempts={} payload_bytes={} samples={}",
                attempts + 1,
                payload.len(),
                rx_signal.len(),
            );

            generated.push(GeneratedVector {
                id: id.clone(),
                level,
                channel: channel.id(),
                seed,
                attempts: attempts + 1,
                payload_hex: to_hex(&payload),
                wav_file: wav_name,
                snr_db: if matches!(channel, Channel::Clean) {
                    None
                } else {
                    Some(snr_db)
                },
                cfo_hz: if matches!(channel, Channel::SsbCfo) {
                    Some(SSB_CFO_HZ)
                } else {
                    None
                },
                // Always true: `found` above only sets when a seed actually
                // decoded correctly (there is no longer a deliberately-failing
                // combination -- see this generator's 2026-07-26 correction in
                // the module doc's "Level 9's exception" section).
                expected_decode_ok: true,
            });
        }
    }

    let mut manifest_toml = String::new();
    manifest_toml.push_str("# Golden decode-regression vectors (Task 8, decision 9c).\n");
    manifest_toml.push_str(
        "# Generated by `cargo run -p coppa-bench --release --example golden_vectors_gen`.\n",
    );
    manifest_toml.push_str(
        "# Each vector: a 48kHz/16-bit-PCM WAV of one Coppa frame through a fixed channel\n",
    );
    manifest_toml.push_str(
        "# condition, plus the payload it must decode back to exactly (see golden_vectors.rs).\n\n",
    );
    for v in &generated {
        manifest_toml.push_str("[[vectors]]\n");
        manifest_toml.push_str(&format!("id = \"{}\"\n", v.id));
        manifest_toml.push_str(&format!("level = {}\n", v.level));
        manifest_toml.push_str(&format!("channel = \"{}\"\n", v.channel));
        manifest_toml.push_str(&format!("seed = {}\n", v.seed));
        manifest_toml.push_str(&format!("seed_attempts = {}\n", v.attempts));
        if let Some(snr) = v.snr_db {
            manifest_toml.push_str(&format!("snr_db = {snr}\n"));
        }
        if let Some(cfo) = v.cfo_hz {
            manifest_toml.push_str(&format!("cfo_hz = {cfo}\n"));
        }
        manifest_toml.push_str(&format!("payload_hex = \"{}\"\n", v.payload_hex));
        manifest_toml.push_str(&format!("wav_file = \"{}\"\n", v.wav_file));
        manifest_toml.push_str("sample_rate = 48000\n");
        manifest_toml.push_str(&format!(
            "expected_decode_ok = {}\n\n",
            v.expected_decode_ok
        ));
    }

    let manifest_path = out_dir.join("manifest.toml");
    std::fs::write(&manifest_path, manifest_toml).expect("write manifest.toml");

    let ok_count = generated.iter().filter(|v| v.expected_decode_ok).count();
    println!(
        "\nWrote {} golden vectors to {} ({} expected to decode, {} documented known-failures)",
        generated.len(),
        out_dir.display(),
        ok_count,
        generated.len() - ok_count,
    );
    let expected_total = LEVELS.len() * CHANNELS.len();
    if generated.len() != expected_total {
        eprintln!(
            "WARNING: expected {expected_total} vectors, only generated {} -- see WARNINGs above",
            generated.len()
        );
        std::process::exit(1);
    }
}
