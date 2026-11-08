//! Golden-vector generator (Task 8, decision 9c; extended by COP-129): writes
//! the reference WAV files + a manifest under `testdata/golden/`, used by
//! `crates/coppa-protocol/tests/golden_vectors.rs` and
//! `crates/coppa-cli/tests/rx_golden.rs` as a frozen decode regression
//! corpus -- a future PHY/FEC change that silently breaks decoding of these
//! exact frames is now a visible, committed-test failure, not something only
//! a full bench sweep would notice.
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
//! The declared set is `specs()`, 42 vectors in three families. Every WAV is
//! 48 kHz / 16-bit PCM mono (`hound`, matching `coppa_audio::file_backend`'s
//! read/write conventions but 16-bit int rather than that module's
//! `WavSink`'s 32-bit float, per this task's brief).
//!
//! - **Level grid (34):** levels 1-8 x channels {clean, awgn@12dB, poor@25dB,
//!   ssb+cfo}, plus level 9 x {clean, awgn}. Level 9 is VHF-only
//!   (`docs/adr/009-cop24-speed-ladder-renumber.md`), so it has no `poor25`
//!   or `ssbcfo` vector; `L9_ssbcfo` found no decoding seed in 500 attempts
//!   (COP-129 research). The original 20 (levels {1, 2, 5, 6, 8}) date from
//!   Task 8; COP-129 added levels 3, 4, 7 and 9. Each is one single-codeword
//!   RV0 frame carrying a full-capacity payload.
//! - **Multi-codeword (3):** `L4_awgn12_cw3` (532 bytes), `L7_awgn12_cw3`
//!   (533 bytes) and `L9_awgn12_cw8` (1577 bytes). No payload length is a
//!   multiple of the codeword count, so each exercises SPEC §9.1's `+1`
//!   split; L4 sits below and L7/L9 above the level-5 cross-codeword
//!   interleave gate (SPEC §9.3). The manifest records `codewords = N`.
//! - **Retransmission (5):** `L9_awgn12_rtx_rv1`/`_rv2`/`_rv3` (IR-HARQ, RV0
//!   then RVk at 19 dB), `L9_awgn12_rtx_rv0` (Chase, RV0 then RV0 at 20 dB,
//!   the form production sends) and `L4_awgn12_rtx_rv2` (the HF-level pair,
//!   3 dB). The manifest records `rv_sequence = [0, k]`. These are
//!   single-codeword because multi-codeword frames do no cross-retransmission
//!   combining (`docs/LIMITATIONS.md`, ADR-007).
//!
//! ## Retransmission WAV layout
//!
//! One WAV holds the whole on-air sequence: 100 ms (4,800 samples) of
//! silence, the RV0 frame, 100 ms of silence, the retransmission frame, then
//! 100 ms of silence. Both frames carry the same payload and `seq_num` 0.
//! AWGN covers the whole buffer, gaps included, referenced to the first
//! frame's mean power. The generator only accepts a seed where the first
//! frame alone decodes to nothing, so a receiver passes the vector only by
//! combining soft values across the two frames (SPEC §5.3). The batch
//! `CoppaTransceiver::receive` call is single-frame, so only the streaming
//! receiver (and `coppa rx`) decodes these.
//!
//! ## Additive generation
//!
//! Regenerating on a different machine does not reproduce the committed
//! WAVs bit for bit (1-LSB differences, and a different `L5_awgn12` seed;
//! COP-129 research §5). So this generator never rewrites a vector the
//! manifest already lists: if `testdata/golden/manifest.toml` exists, every
//! id in it is kept as is, only the comment header above the first
//! `[[vectors]]` is rewritten, and missing ids are generated and appended.
//! To regenerate one vector, delete its manifest entry and rerun. The
//! process exits 1 if any declared id is missing from the final manifest.
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
//! ## Level 8's exception -- CORRECTED 2026-07-26 (was a bench bug, not a codec limitation)
//!
//! Level 8 (64QAM 2/3) previously needed non-literal operating points here,
//! (old wire level 9 before COP-24) and `L9_poor25` was committed with `expected_decode_ok = false` as a
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
//! (regardless of outcome), then re-measured level 8 directly (100-300
//! trials/point, fresh transceiver + eviction per trial, bypassing this
//! generator's seed-search framing entirely):
//!
//! - **AWGN** (`vhf_wide`, this level's default profile): a completely
//!   ordinary waterfall -- 100% FER at 12/15 dB (genuinely below threshold),
//!   6% at 18 dB, then a clean 0% from 21-27 dB, 1% at 30 dB. Real threshold
//!   is ~18-21 dB, not "30 dB and still mostly failing." `LEVEL8_AWGN_SNR_DB`
//!   is corrected to 24 dB (comfortably clear, matching the margin other
//!   levels' reference SNRs use).
//! - **ssb+cfo** (`hf_standard` forced, 15 Hz CFO): even cleaner -- 0% FER
//!   at every tested point from 18-33 dB. `LEVEL8_SSBCFO_SNR_DB` corrected
//!   to 24 dB.
//! - **Watterson Poor** (`hf_standard` forced, matching this combination's
//!   profile override): genuinely bad, but NOT zero -- 86-91% FER at 25 dB
//!   (100-300 trials), flat out to 54 dB. A ~9-14% real success rate is
//!   easily enough for a 500-attempt seed search to find a decodable frame
//!   (verified: it does). This IS a real, severe channel-estimation-class
//!   gap for 64QAM under deep HF fading -- consistent with docs/LIMITATIONS.md's
//!   "Phase 2 channel estimation" and "Turbo re-estimation... concentrated
//!   on low-order modulation" limitations -- just not the literal "0% at any
//!   SNR" this doc previously claimed. `L8_poor25` is now generated like
//!   every other combination (searched, decoded, `expected_decode_ok = true`)
//!   rather than hand-classified as an unconditional failure.
//!
//! Root-causing *why* level 8 specifically (vs. levels 1/2/5/6 here) is worse
//! under Watterson Poor is still out of scope for this generator -- that part
//! of the original diagnosis stands, just corrected from "impossible" to
//! "real but not universal."
//!
//! ## Seed selection
//!
//! This generator searches a small, deterministic sequence of payload seeds
//! (see `MAX_SEED_ATTEMPTS`) for one that this commit's codec actually
//! decodes correctly at that operating point, for every combination
//! (including `L8_poor25` as of the correction above) -- this is meant to be
//! a KNOWN-GOOD regression corpus (a future regression is "this exact frame
//! no longer decodes", not "we rolled dice and got unlucky at generation
//! time"). The manifest records which seed (and how many attempts) each
//! entry needed. A combination that exhausts `MAX_SEED_ATTEMPTS` without a
//! clean decode is skipped with a warning rather than silently committed --
//! see the bug this replaced above for why that skip condition itself needs
//! a HARQ-buffer-eviction guarantee to be trustworthy.
//!
//! Since COP-129 every attempt decodes with fresh receivers (a new
//! `CoppaTransceiver` and a new `StreamingReceiver`), which replaces the
//! `harq_evict` guard: no attempt can inherit another attempt's IR-HARQ
//! accumulator. A seed is accepted only if the receivers the tests use
//! decode it from the 16-bit-quantized samples the WAV stores: the batch
//! receiver returns the exact payload (single-frame specs), and the
//! streaming receiver, fed 4,800-sample chunks plus 8,192 trailing zeros,
//! returns exactly one frame with the exact payload.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::streaming::{DecodedFrame, StreamingReceiver};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// Levels that get the full four-channel grid.
const GRID_LEVELS: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8];

/// Level 9 is VHF-only (ADR-009), so it gets only the channels that keep
/// its own `vhf_wide` profile.
const LEVEL9_CHANNELS: &[Channel] = &[Channel::Clean, Channel::Awgn12];

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
/// +-50 Hz two-stage-acquisition tolerance (docs/LIMITATIONS.md).
const SSB_CFO_HZ: f32 = 15.0;

/// AWGN operating point for the `awgn12` channel at levels 7, 8 and 9.
/// Level 8's value was corrected 2026-07-26 (see module doc's "Level 8's
/// exception" section): the literal 12 dB grid value is too low for 64QAM
/// 2/3, but the level's real threshold is an ordinary ~18-21 dB AWGN -- NOT
/// the ~30 dB "late, steep, seed-dependent waterfall" this generator
/// previously (incorrectly) characterized. 24 dB is comfortably above that
/// real threshold (verified 0% FER over 100 trials), not a marginal
/// seed-search anchor. COP-129 applied the same value to levels 7 and 9 on
/// `vhf_wide`: at 12 dB neither found a seed in 500 attempts, at 24 dB both
/// decode at attempt 1.
const HIGH_ORDER_AWGN_SNR_DB: f32 = 24.0;
/// Level 8's ssb+cfo operating point (see `HIGH_ORDER_AWGN_SNR_DB`).
const LEVEL8_SSBCFO_SNR_DB: f32 = 24.0;

/// Operating points for the retransmission vectors: where the first frame
/// alone fails to decode (COP-129 plan probe: 30 of 30 at L9 19 dB and 20 dB,
/// 28 of 30 at L4 3 dB) and the pair still combines.
const L9_IR_RTX_SNR_DB: f32 = 19.0;
const L9_CHASE_RTX_SNR_DB: f32 = 20.0;
const L4_IR_RTX_SNR_DB: f32 = 3.0;

/// Silence before, between and after the frames of a retransmission vector:
/// 100 ms at 48 kHz.
const RTX_GAP_SAMPLES: usize = 4800;

/// Streaming chunk size, mirroring `golden_vectors.rs` (100 ms at 48 kHz,
/// what `coppa rx` and the daemon's audio loop push).
const STREAM_CHUNK: usize = 4800;
/// Trailing zeros after each WAV, mirroring `golden_vectors.rs` and the
/// CLI's `TRAILING_FLUSH_SAMPLES`.
const TRAILING_FLUSH_SAMPLES: usize = 8192;

/// Deterministic payload seeds to try per combination before giving up.
const MAX_SEED_ATTEMPTS: u64 = 500;

/// One declared golden vector.
struct Spec {
    id: String,
    level: u8,
    channel: Channel,
    codewords: u8,
    payload_len: usize,
    /// One frame per element, in on-air order. `[0]` for a single frame.
    rv_sequence: &'static [u8],
    snr_db: f32,
    base_seed: u64,
}

impl Spec {
    fn is_retransmission(&self) -> bool {
        self.rv_sequence.len() > 1
    }
}

/// Base seed for a grid id: the original Task 8 per-(level, channel) mix,
/// kept so the formula still describes the committed grid entries.
fn grid_base_seed(level: u8, channel: Channel) -> u64 {
    let mut h = 0x9E3779B97F4A7C15u64;
    h ^= level as u64;
    h = h.wrapping_mul(0x100_0000_01B3);
    h ^= channel
        .id()
        .bytes()
        .fold(0u64, |a, b| a.wrapping_add(b as u64));
    h
}

/// Base seed for a non-grid id: FNV-1a over the whole id string, so
/// `L9_awgn12_rtx_rv2` and `L9_awgn12_rtx_rv3` search different seeds.
fn id_base_seed(id: &str) -> u64 {
    id.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01B3)
    })
}

fn full_payload(level: u8) -> usize {
    mode_for_level(level).expect("valid level").payload_bytes()
}

/// Every vector the corpus must hold, in manifest order for new entries.
fn specs() -> Vec<Spec> {
    let mut out = Vec::new();
    let grid = GRID_LEVELS
        .iter()
        .flat_map(|&l| CHANNELS.iter().map(move |&c| (l, c)))
        .chain(LEVEL9_CHANNELS.iter().map(|&c| (9u8, c)));
    for (level, channel) in grid {
        out.push(Spec {
            id: format!("L{level}_{}", channel.id()),
            level,
            channel,
            codewords: 1,
            payload_len: full_payload(level),
            rv_sequence: &[0],
            snr_db: awgn_snr_for(level, channel),
            base_seed: grid_base_seed(level, channel),
        });
    }

    // Multi-codeword: (level, codewords, payload_len). None of the lengths
    // is a multiple of the codeword count (SPEC §9.1's `+1` split).
    for &(level, codewords, payload_len) in &[(4u8, 3u8, 532usize), (7, 3, 533), (9, 8, 1577)] {
        let channel = Channel::Awgn12;
        let id = format!("L{level}_{}_cw{codewords}", channel.id());
        out.push(Spec {
            base_seed: id_base_seed(&id),
            id,
            level,
            channel,
            codewords,
            payload_len,
            rv_sequence: &[0],
            snr_db: awgn_snr_for(level, channel),
        });
    }

    // Retransmission: (level, rv_sequence, snr_db).
    let rtx: &[(u8, &'static [u8], f32)] = &[
        (9, &[0, 2], L9_IR_RTX_SNR_DB),
        (9, &[0, 3], L9_IR_RTX_SNR_DB),
        (9, &[0, 1], L9_IR_RTX_SNR_DB),
        (9, &[0, 0], L9_CHASE_RTX_SNR_DB),
        (4, &[0, 2], L4_IR_RTX_SNR_DB),
    ];
    for &(level, rv_sequence, snr_db) in rtx {
        let channel = Channel::Awgn12;
        let last_rv = *rv_sequence.last().expect("non-empty rv_sequence");
        let id = format!("L{level}_{}_rtx_rv{last_rv}", channel.id());
        out.push(Spec {
            base_seed: id_base_seed(&id),
            id,
            level,
            channel,
            codewords: 1,
            payload_len: full_payload(level),
            rv_sequence,
            snr_db,
        });
    }
    out
}

fn make_header(level: u8, codewords: u8, rv: u8, payload_len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: rv & 0x03,
        speed_level: level,
        seq_num: 0,
        payload_len,
        codewords,
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
/// doc: levels 7-9 need a documented, verified exception for `awgn12`, and
/// level 8 for `ssbcfo`.
fn awgn_snr_for(level: u8, channel: Channel) -> f32 {
    match (level, channel) {
        (7..=9, Channel::Awgn12) => HIGH_ORDER_AWGN_SNR_DB,
        (8, Channel::SsbCfo) => LEVEL8_SSBCFO_SNR_DB,
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

/// One f32 sample to the 16-bit integer `write_wav_i16` stores.
fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// The samples a reader gets back from the WAV: `write_wav_i16`'s rounding
/// followed by the tests' `/ 32768` full-scale normalization.
fn quantize_i16(samples: &[f32]) -> Vec<f32> {
    samples
        .iter()
        .map(|&s| to_i16(s) as f32 / 32768.0)
        .collect()
}

/// Decode `samples` the way `golden_vectors.rs`'s streaming test does: a fresh
/// receiver, `STREAM_CHUNK`-sample pushes and `TRAILING_FLUSH_SAMPLES` zeros.
fn stream_decode(profile: CoppaProfile, samples: &[f32]) -> Vec<DecodedFrame> {
    let mut buf = samples.to_vec();
    buf.extend(std::iter::repeat_n(0.0f32, TRAILING_FLUSH_SAMPLES));
    let mut rx = StreamingReceiver::new(profile, 1);
    let mut decoded = Vec::new();
    for chunk in buf.chunks(STREAM_CHUNK) {
        decoded.extend(rx.push_samples(chunk));
    }
    decoded
}

/// Build the RX-side signal for `spec` with this attempt's `payload`.
/// Returns the samples and, for a retransmission, the sample index where the
/// gap after the first frame ends.
fn build_signal(
    spec: &Spec,
    tx: &CoppaTransceiver,
    payload: &[u8],
    seed: u64,
) -> (Vec<f32>, Option<usize>) {
    let frame = |rv: u8| {
        let header = make_header(spec.level, spec.codewords, rv, spec.payload_len as u16);
        tx.transmit(&header, payload)
            .expect("payload within this level's capacity")
    };
    if !spec.is_retransmission() {
        let clean = frame(spec.rv_sequence[0]);
        return (apply_channel(spec.channel, &clean, spec.snr_db, seed), None);
    }

    assert!(
        spec.channel == Channel::Awgn12,
        "{}: retransmission vectors are AWGN-only",
        spec.id
    );
    let mut clean = vec![0.0f32; RTX_GAP_SAMPLES];
    let mut first_end = None;
    let mut p_ref = 0.0f32;
    for (i, &rv) in spec.rv_sequence.iter().enumerate() {
        let f = frame(rv);
        if i == 0 {
            p_ref = coppa_channel::mean_power(&f);
        }
        clean.extend_from_slice(&f);
        clean.extend(std::iter::repeat_n(0.0f32, RTX_GAP_SAMPLES));
        if i == 0 {
            first_end = Some(clean.len());
        }
    }
    let noisy = coppa_channel::awgn_ref_seeded(
        &clean,
        spec.snr_db,
        p_ref,
        SAMPLE_RATE as f32,
        seed ^ 0x5555,
    );
    (noisy, first_end)
}

/// Decision 6 of the COP-129 plan: accept a seed only if the receivers the
/// tests use decode it from the quantized samples, each built fresh.
fn accepts(spec: &Spec, payload: &[u8], quantized: &[f32], first_end: Option<usize>) -> bool {
    let profile = profile_for(spec.level, spec.channel);
    let stream_ok = |frames: &[DecodedFrame], rv: u8| {
        frames.len() == 1
            && frames[0].payload == payload
            && frames[0].header.speed_level == spec.level
            && frames[0].header.codewords == spec.codewords
            && frames[0].header.fec_type & 0x03 == rv
    };

    if let Some(first_end) = first_end {
        // The first frame alone must give nothing, so only combining decodes.
        if !stream_decode(profile.clone(), &quantized[..first_end]).is_empty() {
            return false;
        }
        let last_rv = *spec.rv_sequence.last().expect("non-empty rv_sequence");
        return stream_ok(&stream_decode(profile, quantized), last_rv);
    }

    let batch_ok = match CoppaTransceiver::new(profile.clone(), 1).receive(quantized) {
        Ok((header, bytes, _lvl)) => {
            header.speed_level == spec.level
                && header.codewords == spec.codewords
                && bytes == payload
        }
        Err(_) => false,
    };
    batch_ok && stream_ok(&stream_decode(profile, quantized), spec.rv_sequence[0])
}

fn write_wav_i16(path: &Path, samples: &[f32], sample_rate: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create golden WAV");
    for &s in samples {
        writer
            .write_sample(to_i16(s))
            .expect("write golden WAV sample");
    }
    writer.finalize().expect("finalize golden WAV");
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Ids already in a manifest. Only `id` is read; every other key is
/// preserved byte for byte by the additive writer.
#[derive(serde::Deserialize)]
struct IdManifest {
    #[serde(default)]
    vectors: Vec<IdEntry>,
}

#[derive(serde::Deserialize)]
struct IdEntry {
    id: String,
}

fn manifest_ids(text: &str) -> Vec<String> {
    let parsed: IdManifest = toml::from_str(text).expect("parse existing manifest.toml");
    parsed.vectors.into_iter().map(|v| v.id).collect()
}

const MANIFEST_HEADER: &str = "\
# Golden decode-regression vectors (Task 8, decision 9c; extended by COP-129).
# Generated by `cargo run -p coppa-bench --release --example golden_vectors_gen`.
# The generator is additive: it keeps every entry below as is and appends only
# missing ids. Delete an entry to regenerate it.
# Each vector: a 48kHz/16-bit-PCM WAV through a fixed channel condition, plus the
# payload it must decode back to exactly (see golden_vectors.rs and rx_golden.rs).
# Optional keys: `codewords` (default 1) is the frame's LDPC codeword count.
# `rv_sequence` (default [0]) lists one frame per element in on-air order: the WAV
# holds 100 ms of silence, then each frame followed by 100 ms of silence, all with
# the same payload and seq_num 0. A receiver decodes it only by combining frames.

";

/// Format one manifest block in the existing key order. `codewords` and
/// `rv_sequence` appear only when they differ from their defaults.
fn manifest_block(spec: &Spec, seed: u64, attempts: u64, payload: &[u8]) -> String {
    let mut b = String::new();
    b.push_str("[[vectors]]\n");
    b.push_str(&format!("id = \"{}\"\n", spec.id));
    b.push_str(&format!("level = {}\n", spec.level));
    b.push_str(&format!("channel = \"{}\"\n", spec.channel.id()));
    b.push_str(&format!("seed = {seed}\n"));
    b.push_str(&format!("seed_attempts = {attempts}\n"));
    if spec.channel != Channel::Clean {
        b.push_str(&format!("snr_db = {}\n", spec.snr_db));
    }
    if spec.channel == Channel::SsbCfo {
        b.push_str(&format!("cfo_hz = {SSB_CFO_HZ}\n"));
    }
    if spec.codewords > 1 {
        b.push_str(&format!("codewords = {}\n", spec.codewords));
    }
    if spec.is_retransmission() {
        b.push_str(&format!("rv_sequence = {:?}\n", spec.rv_sequence));
    }
    b.push_str(&format!("payload_hex = \"{}\"\n", to_hex(payload)));
    b.push_str(&format!("wav_file = \"{}.wav\"\n", spec.id));
    b.push_str(&format!("sample_rate = {SAMPLE_RATE}\n"));
    // Always true: a block is only written for a seed that decoded.
    b.push_str("expected_decode_ok = true\n\n");
    b
}

fn main() {
    let out_dir = PathBuf::from("testdata/golden");
    std::fs::create_dir_all(&out_dir).expect(
        "create testdata/golden (run this from the workspace root: \
         `cargo run -p coppa-bench --release --example golden_vectors_gen`)",
    );
    let manifest_path = out_dir.join("manifest.toml");

    // Everything from the first `[[vectors]]` on is kept byte for byte.
    let existing_text = std::fs::read_to_string(&manifest_path).unwrap_or_default();
    let existing_ids: HashSet<String> = manifest_ids(&existing_text).into_iter().collect();
    let mut entries = match existing_text.find("[[vectors]]") {
        Some(i) => existing_text[i..].to_string(),
        None => String::new(),
    };
    if !entries.is_empty() && !entries.ends_with("\n\n") {
        entries.push_str(if entries.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        });
    }

    let specs = specs();
    let mut kept = 0usize;
    let mut written = 0usize;
    for spec in &specs {
        if existing_ids.contains(&spec.id) {
            println!("{}: kept", spec.id);
            kept += 1;
            continue;
        }

        let tx = CoppaTransceiver::new(profile_for(spec.level, spec.channel), 1);
        let mut found = None;
        for attempt in 0..MAX_SEED_ATTEMPTS {
            let seed = spec.base_seed.wrapping_add(attempt);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload: Vec<u8> = (0..spec.payload_len).map(|_| rng.random::<u8>()).collect();
            let (rx_signal, first_end) = build_signal(spec, &tx, &payload, seed);
            let quantized = quantize_i16(&rx_signal);
            if accepts(spec, &payload, &quantized, first_end) {
                found = Some((seed, attempt + 1, payload, rx_signal));
                break;
            }
        }

        let Some((seed, attempts, payload, rx_signal)) = found else {
            eprintln!(
                "WARNING: {}: no seed in {MAX_SEED_ATTEMPTS} attempts decoded cleanly -- SKIPPED",
                spec.id
            );
            continue;
        };

        write_wav_i16(
            &out_dir.join(format!("{}.wav", spec.id)),
            &rx_signal,
            SAMPLE_RATE,
        );
        let mut extra = String::new();
        if spec.codewords > 1 {
            extra.push_str(&format!(" codewords={}", spec.codewords));
        }
        if spec.is_retransmission() {
            extra.push_str(&format!(" rv_sequence={:?}", spec.rv_sequence));
        }
        println!(
            "{}: seed=0x{seed:016X} attempts={attempts} payload_bytes={} samples={}{extra}",
            spec.id,
            payload.len(),
            rx_signal.len(),
        );
        entries.push_str(&manifest_block(spec, seed, attempts, &payload));
        written += 1;
    }

    let manifest_text = format!("{MANIFEST_HEADER}{entries}");
    std::fs::write(&manifest_path, &manifest_text).expect("write manifest.toml");

    let final_ids: HashSet<String> = manifest_ids(&manifest_text).into_iter().collect();
    println!(
        "\nKept {kept}, wrote {written} golden vectors to {} ({} in manifest)",
        out_dir.display(),
        final_ids.len(),
    );
    let missing: Vec<&str> = specs
        .iter()
        .map(|s| s.id.as_str())
        .filter(|id| !final_ids.contains(*id))
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "WARNING: {} declared vector(s) missing from the manifest: {} -- see WARNINGs above",
            missing.len(),
            missing.join(", ")
        );
        std::process::exit(1);
    }
}
