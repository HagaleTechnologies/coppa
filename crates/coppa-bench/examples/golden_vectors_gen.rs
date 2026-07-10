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
//! ## Level 9's exception, and why it's real (not a generator bug)
//!
//! Level 9 (64QAM 2/3) needed real, DIFFERENT operating points than the
//! literal 12 dB / 25 dB grid, verified with direct `coppa-bench` sweeps
//! (not guessed) while building this generator:
//!
//! - **AWGN**: 100% frame loss at every SNR from 12-24 dB (3 kHz-referenced
//!   convention, this crate's `awgn_ref_seeded`). CORRECTED (a Task 8 review
//!   caught this): this does NOT "clear to FER=0 at 30 dB" as an earlier
//!   version of this doc claimed. Direct re-measurement at 30 dB with three
//!   well-separated seeds gives 50%/86%/98% FER (50 trials each) -- the
//!   opposite of a clean waterfall. More tellingly: holding one seed fixed
//!   and sweeping SNR from 30 dB up to 60 dB (near-noiseless) leaves the
//!   frame-error count *exactly* unchanged -- 25/50 at every one of
//!   30/33/36/39/42/45/48/60 dB for one seed, 43/50 unchanged the same way
//!   for another -- proving this is not a noise-limited waterfall at all.
//!   Above ~24-30 dB, whether a given (payload, noise-realization) pair
//!   decodes is governed by a payload-dependent decode floor, not by SNR
//!   headroom; raising SNR further does not help. `LEVEL9_AWGN_SNR_DB`
//!   (30 dB) is therefore NOT a "verified clean operating point" -- it is a
//!   high-SNR regime where thermal noise is no longer the dominant failure
//!   mode, so the seed search below (which varies PAYLOADS, not SNR) can and
//!   does reliably land on one of the apparently-common payloads that falls
//!   on the "decodes" side of that floor (this corpus's `L9_awgn12` needed
//!   only 1 attempt). That committed seed is directly verified to decode
//!   correctly and is frozen into a WAV file (no re-randomization at test
//!   time), so it will not spontaneously flip on its own -- but it sits in a
//!   regime with real, structural, seed-to-seed instability, not a
//!   comfortably-passing "typical" point, so a future numerical tweak to the
//!   FEC/demod path is not guaranteed to leave it decoding. A stable,
//!   comfortably-low-FER SNR was searched for (18-60 dB, multiple seeds) and
//!   not found -- re-deriving one is not simply a matter of picking a better
//!   dB value, since the failure mode above is not SNR-driven. Root-causing
//!   the floor itself (plausibly connected to the already-documented LDPC/
//!   turbo-re-estimation limitations in CLAUDE.md, but not confirmed) is out
//!   of scope for this benchmark/golden-vector task.
//! - **Watterson Poor**: 100% frame loss at EVERY tested SNR up to 54 dB.
//!   Re-tested under Watterson GOOD (the mildest fading preset) up to 48 dB:
//!   still 100% frame loss. This is a genuine, structural, already-known-class
//!   gap (see CLAUDE.md's "Phase 2 channel estimation" and "Turbo
//!   re-estimation... concentrated on low-order modulation" limitations,
//!   though this generator's direct measurement is a more extreme instance
//!   than either of those entries states outright) -- 64QAM 2/3 order simply
//!   does not survive this codec's current Watterson channel estimation at
//!   ANY SNR, not something a different seed or a higher SNR fixes. Rather
//!   than quietly drop this combination or fake a pass, `L9_poor25` is
//!   generated anyway with `expected_decode_ok = false` and the exact
//!   verified conditions recorded -- a real, honest regression tripwire: if a
//!   future channel-estimation fix ever makes this decode, the integration
//!   test will flag the manifest as stale (a good thing to notice).
//! - **ssb+cfo**: no fading in this combination (AWGN + SSB passband + CFO
//!   only), so once `hf_standard` is forced, it behaves like the AWGN case
//!   above -- including, presumably, the same payload-dependent floor rather
//!   than a clean SNR waterfall (not independently re-swept to the same
//!   depth as AWGN above, since the underlying mechanism appears shared).
//!   `LEVEL9_SSBCFO_SNR_DB` is bumped accordingly, and its one committed seed
//!   is directly verified to decode.
//!
//! This is not root-caused further here (out of scope for a benchmark/golden-
//! vector task) -- see the module doc's citations for the pre-existing,
//! related known limitations.
//!
//! ## Seed selection
//!
//! For combinations expected to decode, this generator searches a small,
//! deterministic sequence of payload seeds (see `MAX_SEED_ATTEMPTS`) for one
//! that this commit's codec actually decodes correctly at that operating
//! point -- this is meant to be a KNOWN-GOOD regression corpus (a future
//! regression is "this exact frame no longer decodes", not "we rolled dice
//! and got unlucky at generation time"). The manifest records which seed (and
//! how many attempts) each entry needed. For `L9_poor25` (expected to fail,
//! see above), no search is performed -- the base seed is used directly.

use std::path::PathBuf;

use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

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

/// Level 9's AWGN/ssb+cfo operating points (see module doc: the literal
/// 12/20 dB grid values are structurally too low for 64QAM 2/3 -- 100% frame
/// loss below ~24-30 dB). NOT "verified-clean" SNRs: per the module doc's
/// correction, above ~24-30 dB this level's decode outcome is governed by a
/// payload-dependent floor, not by SNR headroom, so these values just place
/// the seed search in the regime where thermal noise stops dominating.
const LEVEL9_AWGN_SNR_DB: f32 = 30.0;
const LEVEL9_SSBCFO_SNR_DB: f32 = 33.0;

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

/// Combinations verified (module doc) to be structurally undecodable at ANY
/// SNR this codec currently supports -- generated anyway (with
/// `expected_decode_ok = false`) as an honest regression tripwire, not skipped
/// and not seed-searched (there is nothing a different seed could fix).
fn known_undecodable(level: u8, channel: Channel) -> bool {
    matches!((level, channel), (9, Channel::Poor25))
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

            let undecodable = known_undecodable(level, channel);
            let attempts_budget = if undecodable { 1 } else { MAX_SEED_ATTEMPTS };

            let mut found = None;
            for attempt in 0..attempts_budget {
                let seed = base_seed.wrapping_add(attempt);
                let mut rng = StdRng::seed_from_u64(seed);
                let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
                let header = make_header(level, payload_bytes as u16);
                let clean = tx
                    .transmit(&header, &payload)
                    .expect("payload within this level's capacity");
                let rx_signal = apply_channel(channel, &clean, snr_db, seed);

                if undecodable {
                    // No search: this combination is a documented, verified
                    // known failure (module doc). Use this seed's WAV as-is.
                    found = Some((seed, attempt, payload, rx_signal, false));
                    break;
                }
                if let Ok((_h, bytes, _lvl)) = tx.receive(&rx_signal) {
                    if bytes.len() >= payload.len() && bytes[..payload.len()] == payload[..] {
                        found = Some((seed, attempt, payload, rx_signal, true));
                        break;
                    }
                }
            }

            let Some((seed, attempts, payload, rx_signal, decoded)) = found else {
                eprintln!(
                    "WARNING: {id}: no seed in {attempts_budget} attempts decoded cleanly -- SKIPPED"
                );
                continue;
            };

            let wav_name = format!("{id}.wav");
            write_wav_i16(&out_dir.join(&wav_name), &rx_signal, SAMPLE_RATE);

            println!(
                "{id}: seed=0x{seed:016X} attempts={} payload_bytes={} samples={} decoded={}",
                attempts + 1,
                payload.len(),
                rx_signal.len(),
                decoded,
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
                expected_decode_ok: decoded,
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
        "# condition, plus the payload it must decode back to exactly (see golden_vectors.rs).\n",
    );
    manifest_toml.push_str(
        "# expected_decode_ok=false entries are DOCUMENTED, VERIFIED known-limitation failures\n",
    );
    manifest_toml.push_str(
        "# (see golden_vectors_gen.rs's module doc), not generator bugs -- kept as a tripwire.\n\n",
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
