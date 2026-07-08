use std::cell::Cell;
use std::collections::HashMap;

use crc::{Crc, CRC_32_ISO_HDLC};
use num_complex::Complex32;

use crate::fec::ldpc::nr_bg2;
use crate::fec::ldpc::{pin_known_pad, rate_match, NrLdpc};
use crate::fec::scrambler::scramble;
use crate::modem::speed_levels::{k_used_for_level, max_payload_for_level, speed_level_components};
use coppa_codec::ofdm::coppa_modem::{CoppaModem, SPEED_LEVELS};
use coppa_codec::ofdm::frame::CoppaHeader;
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::ofdm::CoppaProfile;
use coppa_codec::traits::ConstellationMapper;

/// One-round "turbo" re-estimation soft-symbol closed forms (Task 5).
///
/// When the first LDPC decode attempt fails to converge, its posterior LLRs
/// (over the coded bits) are turned into soft "virtual pilot" symbols -- the
/// same `x̄ = E[x | LLRs]` expectation a genuine pilot symbol would give the
/// channel estimator, just derived from the decoder's current best guess
/// instead of a known TX value -- and fed back into
/// `CoppaModem::reequalize_with_virtual_pilots` as extra weighted
/// observations before a single retry demap/decode.
///
/// All bit-LLR inputs use this codebase's universal convention (see
/// `coppa_codec::traits::ConstellationMapper::demap_soft`'s doc): positive LLR
/// means the bit is more likely 0. Under that convention `E[1-2b] =
/// tanh(L/2)` for a single bit `b` with LLR `L` (standard sigmoid-posterior
/// identity), which is the building block every closed form below composes.
///
/// BPSK/QPSK are transcribed directly from the task brief (already exact/
/// correct there). PAM4 (16-QAM's per-axis 2-bit mapping) and the 64-QAM
/// per-axis (3-bit) form are DERIVED here from the actual Gray tables in
/// `coppa_codec::qam16`/`coppa_codec::qam64` (not transcribed from the
/// brief's intentionally-incomplete sketch) -- see the derivation comments
/// below and the brute-force-verified unit tests in this module's `tests`.
/// BPSK bit 0 -> +1 (matches `coppa_codec::bpsk::BpskMapper`).
fn soft_symbol_bpsk(l: f32) -> Complex32 {
    Complex32::new((l / 2.0).tanh(), 0.0)
}

/// QPSK: `coppa_codec::qpsk::QpskMapper` maps `(b0,b1)` to
/// `SCALE*(1-2*b1, 1-2*b0)` (`b0` sets the imaginary sign, `b1` the real
/// sign -- see that mapper's doc/`CONSTELLATION` table). `l_b0`/`l_b1` are
/// `b0`'s/`b1`'s LLRs.
fn soft_symbol_qpsk(l_b0: f32, l_b1: f32) -> Complex32 {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    Complex32::new(s * (l_b1 / 2.0).tanh(), s * (l_b0 / 2.0).tanh())
}

/// Gray PAM-4 axis expectation (one 16-QAM axis, 2 LLRs, levels `{+-1,+-3}/
/// sqrt(10)`).
///
/// # Derivation from `coppa_codec::qam16::Qam16Mapper`'s actual table
///
/// `Qam16Mapper::bits_to_level(b_msb, b_lsb)` computes `idx = (b_msb<<1)|b_lsb`
/// and returns `LEVEL[idx]*NORM` with `LEVEL = [3,1,-1,-3]`. Writing
/// `x = 1-2*b_msb`, `y = 1-2*b_lsb` (each in `{+1,-1}`), the 4 `(x,y) ->
/// unnormalized level` pairs are `(1,1)->3, (1,-1)->1, (-1,1)->-1,
/// (-1,-1)->-3`. Solving the bilinear system `level = a + b*x + c*y + d*x*y`
/// against these 4 points gives the EXACT identity (no residual/cross term):
/// `level = 2*x + y` (verified: `(1,1)->2+1=3`, `(1,-1)->2-1=1`,
/// `(-1,1)->-2+1=-1`, `(-1,-1)->-2-1=-3`). Since `d=0`, `E[level] = 2*E[x] +
/// E[y] = 2*tanh(l_msb/2) + tanh(l_lsb/2)` is EXACT (not merely a leading-order
/// approximation) under the assumed bit independence -- confirmed against a
/// brute-force `sum_x x*P(x|LLRs)` reference in this module's tests to
/// 1e-6-level agreement (float-rounding only).
fn soft_pam4(l_msb: f32, l_lsb: f32) -> f32 {
    let norm = 1.0 / 10.0f32.sqrt(); // matches qam16::NORM (kept independent to avoid a cross-crate `pub` just for a test constant)
    (2.0 * (l_msb / 2.0).tanh() + (l_lsb / 2.0).tanh()) * norm
}

/// Gray 64-QAM axis expectation (one axis, 3 LLRs, levels `{+-1,+-3,+-5,+-7}/
/// sqrt(42)`) -- the brief's "same per-axis approach with 3 LLRs/axis" for
/// 64-QAM.
///
/// # Derivation from `coppa_codec::qam64::Qam64Mapper`'s actual table
///
/// `Qam64Mapper::bits_to_level(b0,b1,b2)` Gray-encodes `(b0,b1,b2)` through
/// `GRAY_TO_IDX = [0,1,3,2,7,6,4,5]` into `LEVEL = [7,5,3,1,-1,-3,-5,-7]`.
/// Writing `x=1-2*b0, y=1-2*b1, z=1-2*b2`, tabulating all 8 `(x,y,z) ->
/// unnormalized level` points and solving the exact `2^3`-term Walsh-Hadamard
/// expansion `level = a + b*x + c*y + d*z + e*xy + f*xz + g*yz + h*xyz`
/// against them gives `a=b=...=0` except `b=4` (coefficient of `x`), `e=2`
/// (coefficient of `xy`), `h=1` (coefficient of `xyz`) -- i.e. EXACTLY `level
/// = 4*x + 2*x*y + x*y*z = x*(4 + 2*y + y*z)` (verified against all 8 points,
/// e.g. `(1,1,1)->4+2+1=7`, `(1,1,-1)->4+2-1=5`, `(1,-1,1)->4-2-1=1`,
/// `(1,-1,-1)->4-2+1=3`, and the `x=-1` rows negate those). Under independent
/// bits, `E[xy]=E[x]E[y]` and `E[xyz]=E[x]E[y]E[z]`, so `E[level] = E[x]*(4 +
/// 2*E[y] + E[y]*E[z])` is EXACT (again verified against brute force in this
/// module's tests).
fn soft_qam64_axis(l0: f32, l1: f32, l2: f32) -> f32 {
    let norm = 1.0 / 42.0f32.sqrt(); // matches qam64::NORM
    let (s0, s1, s2) = ((l0 / 2.0).tanh(), (l1 / 2.0).tanh(), (l2 / 2.0).tanh());
    s0 * (4.0 + 2.0 * s1 + s1 * s2) * norm
}

/// Exact brute-force soft-symbol expectation `sum_x x * P(x|LLRs)` over all
/// `2^bits_per_symbol` constellation points (independent-bit assumption),
/// used for any modulation without a hand-derived closed form above. Only
/// 8-PSK (this codec's bits_per_symbol=3 modulation, speed level 5) hits this
/// path today: its constant-modulus circular geometry doesn't decompose into
/// independent per-axis product terms the way rectangular QAM's grid does, so
/// there's no small closed form to derive the way there is for BPSK/QPSK/PAM4/
/// 64-QAM. This is still EXACT (same expectation, just evaluated by
/// enumeration instead of a derived formula), and cheap: `bits_per_symbol<=6`
/// everywhere in this codec, so at most 64 constellation points, evaluated
/// only on the rare turbo-retry path (after a first-pass LDPC failure).
fn soft_symbol_generic(mapper: &dyn ConstellationMapper, llrs: &[f32]) -> Complex32 {
    let bps = llrs.len();
    let mut acc = Complex32::new(0.0, 0.0);
    let mut norm = 0.0f32;
    for idx in 0..(1usize << bps) {
        let bits: Vec<u8> = (0..bps)
            .map(|b| ((idx >> (bps - 1 - b)) & 1) as u8)
            .collect();
        let mut p = 1.0f32;
        for (i, &bit) in bits.iter().enumerate() {
            // P(bit=0) = sigmoid(L) = 1/(1+exp(-L)) under this codebase's
            // "positive LLR = bit more likely 0" convention.
            let p0 = 1.0 / (1.0 + (-llrs[i]).exp());
            p *= if bit == 0 { p0 } else { 1.0 - p0 };
        }
        acc += mapper.map(&bits) * p;
        norm += p;
    }
    if norm > 1e-9 {
        acc * (1.0 / norm)
    } else {
        acc
    }
}

/// Build one data carrier's soft "virtual pilot" symbol from its
/// `bits_per_symbol` posterior LLRs, dispatched by bit width. This codec's
/// fixed `SPEED_LEVELS` ladder maps `bits_per_symbol` uniquely to a modulation
/// (1=BPSK, 2=QPSK, 3=8PSK, 4=16-QAM, 6=64-QAM -- see
/// `speed_levels::speed_level_components`), so switching on `llrs.len()` alone
/// is unambiguous for every currently defined speed level without needing a
/// `dyn Any` downcast on `mapper`.
fn soft_symbol(mapper: &dyn ConstellationMapper, llrs: &[f32]) -> Complex32 {
    match llrs.len() {
        1 => soft_symbol_bpsk(llrs[0]),
        2 => soft_symbol_qpsk(llrs[0], llrs[1]),
        4 => Complex32::new(soft_pam4(llrs[0], llrs[1]), soft_pam4(llrs[2], llrs[3])),
        6 => Complex32::new(
            soft_qam64_axis(llrs[0], llrs[1], llrs[2]),
            soft_qam64_axis(llrs[3], llrs[4], llrs[5]),
        ),
        _ => soft_symbol_generic(mapper, llrs),
    }
}

/// Coded block length: this codec's fixed OFDM/interleaver block size,
/// rate-matched down from the NR BG2 mother code for every speed level (see
/// `crate::fec::ldpc::rate_match`). Unchanged in value from the pre-Task-4
/// per-rate 802.11 QC-LDPC codec (Z=81, 24 base columns also gave 1944), but
/// now a fixed rate-matching target rather than an intrinsic per-code
/// property.
const CODED_BLOCK_LEN: usize = 1944;

/// Payload integrity check (Phase 3 Task 1): a CRC-32 (CRC-32/ISO-HDLC, the
/// familiar "zip/gzip/ethernet" polynomial) appended to the application payload
/// on TX (before scrambling/padding into the LDPC info block) and verified on RX
/// (after descrambling/LDPC decode). This closes a real gap the LDPC layer alone
/// doesn't cover: LDPC convergence only means the decoder found *a* valid
/// codeword, not that it's the *right* one -- rare but possible at low SNR (a
/// wrong-codeword convergence looks identical to a correct one from the decoder's
/// point of view). See `CoppaTransceiver::transmit`/`receive_with_metrics` for
/// the two call sites, and `ReceiveError::CrcMismatch` for the RX-side failure
/// this guards.
const PAYLOAD_CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// Number of CRC-32 trailer bytes appended to every payload by `transmit` (see
/// `PAYLOAD_CRC32`'s doc). Also the fixed overhead subtracted in
/// `crate::modem::speed_levels::max_payload_for_level`'s `k_used/8 - 4` formula --
/// `pub(crate)` and referenced from there directly so the two call sites can't
/// silently drift apart.
pub(crate) const PAYLOAD_CRC_LEN: usize = 4;

/// Cached per-speed-level components: building the interleaver and
/// constellation mapper is ~0.105 ms and ~4801 allocs (~525 KB) per call — expensive
/// enough that doing it on every single `transmit`/`receive` (as the pre-Task-7 code
/// did) shows up directly in the per-frame decode budget. All of these depend only
/// on the speed level + this transceiver's fixed profile, so they're built once, in
/// `CoppaTransceiver::new`, for all 9 levels.
struct LevelComponents {
    interleaver: BlockInterleaver,
    /// `ConstellationMapper` is only `: Send`, not `: Send + Sync` (see its
    /// definition in `coppa_codec::traits`), and `speed_level_components`
    /// returns `Box<dyn ConstellationMapper>` (no `Sync`). As a result,
    /// `CoppaTransceiver` — which embeds this cache — is intentionally
    /// `!Sync`. That's fine for `Send`-only use (no `Mutex`/similar needed to
    /// move it across a thread boundary), but a future caller cannot put a
    /// bare `CoppaTransceiver` behind `Arc<CoppaTransceiver>` for shared
    /// concurrent access without wrapping it in a `Mutex` (or similar) first.
    mapper: Box<dyn ConstellationMapper + Send>,
    /// Shortened NR BG2 mother-code info width for this level (Task 4) --
    /// see `crate::modem::speed_levels::k_used_for_level`.
    k_used: usize,
}

pub struct CoppaTransceiver {
    modem: CoppaModem,
    profile: CoppaProfile,
    /// RX-side SSB-audio-band bandpass (250-2850 Hz), mirroring the TX bandpass
    /// already applied in `CoppaModem::modulate_mapped`. Only meaningful for HF
    /// profiles (`phy_mode == 0`) received through an SSB radio's audio chain;
    /// `None` for non-HF profiles (see `CoppaModem::tx_bpf`'s doc for why VHF's
    /// wider carrier band and shorter cyclic prefix are incompatible with this
    /// filter's passband and 300-sample group delay). Reuses the exact same
    /// 601-tap / 250-2850 Hz design already verified by
    /// `coppa_dsp::fir::tests::bandpass_rejects_out_of_band_tones` (>=30 dB
    /// attenuation at 100 Hz and 4 kHz, flat passband at 500 Hz), so no new
    /// tap-count derivation is needed for this filter specifically.
    rx_bpf: Option<coppa_dsp::fir::Fir>,
    /// **One** NR BG2 mother-code LDPC instance (Task 4), shared by every
    /// speed level (rate-matched down per level instead of switching between
    /// per-rate base matrices -- see `crate::fec::ldpc::NrLdpc` and
    /// `rate_match`). Building the lifted graph + core-parity inverse is a
    /// one-time cost amortized across every `transmit`/`receive` call, same
    /// spirit as `LevelComponents` below (Task 7).
    ldpc: NrLdpc,
    /// Per-speed-level cached interleaver/mapper/k_used, built eagerly for
    /// all 9 levels in `new` (see `LevelComponents`'s doc).
    codecs: HashMap<u8, LevelComponents>,
    /// One-round turbo re-estimation gate (Task 5): when `true` (the
    /// default), a first-pass LDPC non-convergence triggers one retry --
    /// soft "virtual pilot" symbols built from the failed decode's posterior
    /// LLRs, folded into a channel re-fit
    /// (`CoppaModem::reequalize_with_virtual_pilots`), then one re-demap +
    /// re-decode -- before giving up. `false` reproduces the exact pre-Task-5
    /// behavior (single decode attempt only). See [`Self::with_turbo`].
    turbo: bool,
    /// Count of frames where the turbo retry path actually fired (first-pass
    /// non-convergence with `turbo` enabled), for bench instrumentation (Task
    /// 5's Step 3 gate records firing rate per SNR point). `Cell` because
    /// `receive`/`receive_with_metrics` are `&self` like every other method on
    /// this type.
    turbo_attempts: Cell<u64>,
    /// Count of `turbo_attempts` where the retry decode actually converged
    /// (i.e. the frame would otherwise have returned `LdpcNotConverged`, but
    /// didn't). This is the LDPC-level "rescue" count -- `turbo_rescues() /
    /// turbo_attempts()` is the rescue rate bench instrumentation needs to
    /// measure turbo's effect independent of any FER-threshold confound (see
    /// the Task 5 report's discussion of the Watterson-Poor channel's
    /// pre-existing FER floor, where a straight FER@10%-threshold comparison
    /// is undefined for either turbo setting but a rescue rate is still
    /// directly measurable).
    turbo_rescues: Cell<u64>,
}

#[derive(Debug)]
pub enum ReceiveError {
    SyncFailed,
    HeaderCorrupt,
    LdpcNotConverged { iterations: usize },
    CrcMismatch,
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SyncFailed => write!(f, "preamble synchronization failed"),
            Self::HeaderCorrupt => write!(f, "header could not be parsed"),
            Self::LdpcNotConverged { iterations } => {
                write!(
                    f,
                    "LDPC decoder did not converge after {} iterations",
                    iterations
                )
            }
            Self::CrcMismatch => write!(f, "CRC mismatch on decoded payload"),
        }
    }
}

impl std::error::Error for ReceiveError {}

/// TX-side errors from [`CoppaTransceiver::transmit`] (Phase 3 Task 1).
#[derive(Debug, PartialEq, Eq)]
pub enum TransmitError {
    /// `payload.len()` exceeded this speed level's capacity
    /// (`crate::modem::speed_levels::max_payload_for_level`). Oversized payloads
    /// are a hard error now -- the pre-Task-1 codec silently truncated them
    /// instead, which is a data-loss footgun a caller can't detect.
    PayloadTooLarge { max: usize },
}

impl std::fmt::Display for TransmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge { max } => {
                write!(
                    f,
                    "payload too large for this speed level (max {max} bytes)"
                )
            }
        }
    }
}

impl std::error::Error for TransmitError {}

/// Median of the per-carrier noise-variance estimates, used as the fallback
/// noise variance for carriers with a missing or degenerate (near-zero)
/// estimate -- see the call site in `receive_with_metrics` for why a
/// frame-local median is a better fallback than a fixed constant.
///
/// Returns `1.0` (a neutral value: neither artificially confident nor
/// artificially flat) if `noise_vars` is empty, since there is then no
/// frame-local data to derive a fallback from at all.
fn median_noise_variance(noise_vars: &[f32]) -> f32 {
    if noise_vars.is_empty() {
        return 1.0;
    }
    let mut sorted: Vec<f32> = noise_vars.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

impl CoppaTransceiver {
    pub fn new(profile: CoppaProfile, version: u8) -> Self {
        // phy_mode 0 = HF/SSB; mirrors the TX bandpass gate in `CoppaModem::new`.
        let rx_bpf = (profile.phy_mode == 0).then(|| {
            coppa_dsp::fir::Fir::new(coppa_dsp::fir::design_bandpass(
                601,
                profile.sample_rate as f32,
                250.0,
                2850.0,
            ))
        });
        let modem = CoppaModem::new(profile.clone(), version);

        // ONE NR BG2 mother-code instance for every speed level (Task 4) --
        // see `NrLdpc`'s doc.
        let ldpc = NrLdpc::new();

        // Eagerly build every speed level's interleaver/mapper/k_used (see
        // `LevelComponents`'s doc). Reserved/invalid levels (e.g. 8) simply have no
        // entry in the map; `transmit`/`receive` treat a missing entry the same way
        // the old per-call `speed_level_components` lookup treated an `Err`.
        let mut codecs = HashMap::with_capacity(SPEED_LEVELS.len());
        for sl in SPEED_LEVELS.iter() {
            if let (Ok((mapper, _code_rate)), Some(k_used)) =
                (speed_level_components(sl.level), k_used_for_level(sl.level))
            {
                let interleaver = BlockInterleaver::new(CODED_BLOCK_LEN, profile.data_carriers);
                codecs.insert(
                    sl.level,
                    LevelComponents {
                        interleaver,
                        mapper,
                        k_used,
                    },
                );
            }
        }

        Self {
            modem,
            profile,
            rx_bpf,
            ldpc,
            codecs,
            turbo: true,
            turbo_attempts: Cell::new(0),
            turbo_rescues: Cell::new(0),
        }
    }

    /// Enable/disable one-round turbo re-estimation (Task 5, default: on).
    /// Builder-style so bench code can construct a turbo-off transceiver for
    /// an A/B comparison: `CoppaTransceiver::new(profile, version).with_turbo(false)`.
    pub fn with_turbo(mut self, on: bool) -> Self {
        self.turbo = on;
        self
    }

    /// Whether one-round turbo re-estimation is enabled.
    pub fn turbo(&self) -> bool {
        self.turbo
    }

    /// Number of `receive`/`receive_with_metrics` calls (since construction)
    /// where the turbo retry path actually fired (first-pass LDPC
    /// non-convergence with `turbo` enabled) -- bench instrumentation for
    /// Task 5's Step 3 firing-rate-per-SNR-point measurement.
    pub fn turbo_attempts(&self) -> u64 {
        self.turbo_attempts.get()
    }

    /// Number of `turbo_attempts` (first-pass failures with `turbo` on) whose
    /// retry decode actually converged -- the LDPC-level "rescue" count. See
    /// this field's doc for why this is tracked separately from
    /// `turbo_attempts`.
    pub fn turbo_rescues(&self) -> u64 {
        self.turbo_rescues.get()
    }

    /// The OFDM profile this transceiver was built for.
    pub fn profile(&self) -> &CoppaProfile {
        &self.profile
    }

    /// Data (non-pilot) carriers per OFDM symbol, as computed internally by the
    /// wrapped `CoppaModem` (`pilots.num_data()`). Exposed so callers (e.g.
    /// `StreamingReceiver`) can assert this coincides with their own,
    /// independently-derived `profile.data_carriers` — see
    /// `StreamingReceiver::new`'s `debug_assert_eq!`.
    pub fn data_carriers_per_symbol(&self) -> usize {
        self.modem.data_carriers_per_symbol()
    }

    pub fn transmit(
        &self,
        header: &CoppaHeader,
        payload: &[u8],
    ) -> Result<Vec<f32>, TransmitError> {
        let comp = self
            .codecs
            .get(&header.speed_level)
            .expect("invalid speed level in header");
        let k_used = comp.k_used;

        // 0. Hard-reject oversized payloads (Phase 3 Task 1): the pre-Task-1 codec
        //    silently truncated a payload beyond this level's capacity instead --
        //    a data-loss footgun a caller couldn't detect. `max_payload_for_level`
        //    already reserves room for the CRC-32 trailer step 1 appends below.
        let max = max_payload_for_level(header.speed_level).expect("invalid speed level in header");
        if payload.len() > max {
            return Err(TransmitError::PayloadTooLarge { max });
        }

        // 1. Append a CRC-32 trailer over the payload (Phase 3 Task 1): LDPC
        //    convergence alone only means the decoder found *a* valid codeword,
        //    not that it's the *right* one -- this closes that gap. See
        //    `PAYLOAD_CRC32`'s doc.
        let checksum = PAYLOAD_CRC32.checksum(payload);
        let mut payload_with_crc = Vec::with_capacity(payload.len() + PAYLOAD_CRC_LEN);
        payload_with_crc.extend_from_slice(payload);
        payload_with_crc.extend_from_slice(&checksum.to_be_bytes());

        // 2. Build the fixed-width (1760-bit) NR BG2 mother-code info block:
        //    payload+CRC bits (guaranteed to fit within k_used by the oversize
        //    check above -- never truncated), zero-padded out to k_used, then
        //    zero-padded further out to the mother code's full fixed info width
        //    (the shortened tail beyond k_used, never transmitted -- see
        //    `crate::fec::ldpc::rate_match`). The whole 1760-bit block is
        //    scrambled to randomize zero-padding (prevents degenerate LDPC
        //    codewords).
        let mut info_bits = Vec::with_capacity(NrLdpc::INFO_LEN);
        for &byte in &payload_with_crc {
            for shift in (0..8).rev() {
                info_bits.push((byte >> shift) & 1);
            }
        }
        info_bits.resize(k_used, 0u8);
        info_bits.resize(NrLdpc::INFO_LEN, 0u8);
        scramble(&mut info_bits);

        // 3. NR BG2 encode: fixed 1760-bit info -> 8800-bit mother codeword.
        let mother = self.ldpc.encode(&info_bits);

        // 4. Rate match: mother codeword -> CODED_BLOCK_LEN=1944 coded bits
        //    for this level's k_used (RV0 -- Phase 2 doesn't use HARQ-IR).
        let coded_bits = rate_match::rate_match(&mother, k_used, CODED_BLOCK_LEN, 0);

        // 5. Interleave
        let interleaved = comp.interleaver.interleave(&coded_bits);

        // 6. Constellation map
        let symbols = comp.mapper.map_bits(&interleaved);

        // 7. OFDM modulate
        // Look up PAPR target from speed level table
        let sl = SPEED_LEVELS
            .iter()
            .find(|s| s.level == header.speed_level)
            .expect("invalid speed level in header");
        Ok(self
            .modem
            .modulate_mapped(header, &symbols, sl.papr_target_db))
    }

    /// Peek at just the header of a buffered candidate window (samples starting at,
    /// or shortly before, a frame's preamble), without demodulating the payload.
    /// Applies the same RX bandpass `receive` does (HF profiles only) before
    /// delegating to [`CoppaModem::demodulate_header`] — see that method's doc for
    /// why no CFO correction is applied here.
    ///
    /// Unlike [`Self::receive_with_metrics`] (which re-derives its own timing via a
    /// fresh internal `SyncDetector::detect_all`, so tolerates arbitrary leading
    /// margin/silence before the frame), this takes an explicit `data_start` and
    /// does no timing search of its own — so the caller must ensure `samples`
    /// includes enough leading context before the header for this transceiver's
    /// one-shot block RX filter to settle before `data_start` (its group delay is
    /// 300 samples for the 601-tap HF filter; a slice starting exactly at the
    /// frame's preamble, with no leading context at all, shifts the correctly
    /// filtered header later by that much — see `StreamingReceiver::header_peek`
    /// in `coppa-protocol::modem::streaming`, its only caller, for how it handles
    /// this).
    pub fn demodulate_header(&self, samples: &[f32], data_start: usize) -> Option<CoppaHeader> {
        let filtered;
        let samples: &[f32] = match &self.rx_bpf {
            Some(bpf) => {
                filtered = bpf.filter_block(samples);
                &filtered
            }
            None => samples,
        };
        self.modem.demodulate_header(samples, data_start)
    }

    pub fn receive(&self, samples: &[f32]) -> Result<(CoppaHeader, Vec<u8>), ReceiveError> {
        self.receive_with_metrics(samples)
            .map(|(h, p, _snr)| (h, p))
    }

    /// Like [`Self::receive`], but also returns the frame's SNR estimate (dB),
    /// derived from the per-carrier noise-variance estimates the payload equalizer
    /// already produces: `snr_db = 10*log10(1 / mean(noise_vars))`. Added for
    /// `StreamingReceiver`'s `DecodedFrame::snr_db` (Task 7), so the daemon can feed
    /// the rate controller a real per-carrier-noise SNR instead of the crude
    /// whole-buffer RMS proxy it used before (`20*log10(rms) + 40`, flagged
    /// elsewhere as a known hack). `receive` itself is unchanged and still used by
    /// every existing (batch) call site.
    pub fn receive_with_metrics(
        &self,
        samples: &[f32],
    ) -> Result<(CoppaHeader, Vec<u8>, f32), ReceiveError> {
        // 0. RX bandpass: reject out-of-passband noise/interference before demod, mirroring
        // the TX bandpass already applied at modulate time (HF profiles only).
        let filtered;
        let samples: &[f32] = match &self.rx_bpf {
            Some(bpf) => {
                filtered = bpf.filter_block(samples);
                &filtered
            }
            None => samples,
        };

        // 1. Demodulate to soft symbols (coded symbol count derived from header speed level)
        let (header, eq_symbols, mut noise_vars) = self
            .modem
            .demodulate_frame(samples)
            .ok_or(ReceiveError::SyncFailed)?;

        // 2. Resolve speed level components
        let comp = self
            .codecs
            .get(&header.speed_level)
            .ok_or(ReceiveError::HeaderCorrupt)?;

        // 3. Soft demap: convert equalized symbols to LLRs
        let bps = comp.mapper.bits_per_symbol();
        let coded_bits_needed: usize = CODED_BLOCK_LEN;
        let symbols_needed = coded_bits_needed.div_ceil(bps);
        let mut llrs = Vec::with_capacity(coded_bits_needed);

        // Fallback noise variance for carriers with no estimate (or a degenerate
        // near-zero one): the median of the per-carrier estimates we do have, rather
        // than a fixed `0.01`/`0.001` magic constant. A fixed fallback either
        // over-trusts a carrier with no real estimate (too small a variance inflates
        // its LLR magnitude) or under-trusts it relative to the actual channel (too
        // large flattens it) -- the median of this frame's own measured noise floor
        // is a much better prior than an arbitrary constant tuned on a different
        // channel/SNR regime.
        let fallback_nv = median_noise_variance(&noise_vars);

        for (i, &sym) in eq_symbols.iter().take(symbols_needed).enumerate() {
            let nv = match noise_vars.get(i) {
                // A present-but-near-zero estimate is as uninformative as a missing
                // one (dividing by it would blow the LLR up towards +/-infinity), so
                // both cases fall back to the same median-based estimate.
                Some(&v) if v > 1e-6 => v,
                _ => fallback_nv,
            };
            llrs.extend(comp.mapper.demap_soft(sym, nv));
        }
        llrs.truncate(coded_bits_needed);
        llrs.resize(coded_bits_needed, 0.0);

        // Clip LLR magnitudes to prevent numerical overflow in BP decoder
        let llr_clip = 20.0f32;
        for llr in &mut llrs {
            *llr = llr.clamp(-llr_clip, llr_clip);
        }

        // 4. De-interleave
        let deinterleaved = comp.interleaver.deinterleave(&llrs);

        // 5. Rate dematch: scatter the E=1944 received LLRs back into a
        // mother-length (8800) LLR buffer. Positions never observed --
        // including the shortened tail beyond k_used -- are left at 0.0.
        let k_used = comp.k_used;
        let mut dematched = rate_match::rate_dematch(
            &deinterleaved,
            k_used,
            CODED_BLOCK_LEN,
            0,
            NrLdpc::MOTHER_LEN,
        );

        // Known-bit pinning (Task 3, extended in Task 4): info bits beyond the
        // payload are zero-padded then scrambled on TX, so RX knows their
        // exact values -- pin to +/-PIN (effective code shortening; worth
        // 1.5-3 dB on short payloads). Covers *both* the shortened-but-
        // transmitted padding (payload_bits..k_used) and the never-
        // transmitted shortened tail (k_used..KB*ZC, i.e. the dematch
        // buffer's shortened region, which `rate_dematch` otherwise leaves
        // at 0.0) in one pass -- see `pin_known_pad`'s doc.
        const PIN: f32 = 64.0;
        // `payload_len` bytes of application payload + `PAYLOAD_CRC_LEN` bytes of
        // CRC-32 trailer (Phase 3 Task 1) are the real (non-pad) bits `transmit`
        // wrote -- see `PAYLOAD_CRC32`'s doc.
        let payload_bits = (header.payload_len as usize + PAYLOAD_CRC_LEN) * 8;
        if payload_bits > k_used {
            // A corrupted header claiming a payload larger than this level's
            // shortened capacity can't be genuine -- treat as header corruption
            // rather than let `pin_known_pad`'s invariant assert panic.
            return Err(ReceiveError::HeaderCorrupt);
        }
        pin_known_pad(&mut dematched, payload_bits, k_used, PIN);

        // 6. NR BG2 layered decode
        let (posterior, mut decoded_bits, mut converged, mut iterations) =
            self.ldpc.decode_soft_stats(&dematched);

        // 6b. One-round turbo re-estimation (Task 5): on a first-pass non-convergence,
        // build soft "virtual pilot" symbols from the failed decode's posterior LLRs,
        // fold them into a channel re-fit, re-demap, re-pin, and retry the decode
        // exactly once. See `soft_symbol`'s doc for the closed forms and
        // `CoppaModem::reequalize_with_virtual_pilots`'s doc for the re-fit itself.
        if !converged && self.turbo {
            self.turbo_attempts.set(self.turbo_attempts.get() + 1);

            // Posterior -> mother-domain LLRs (strip the always-punctured leading
            // `PUNCTURED_INFO_COLS*ZC` positions -- see `NrLdpc::decode_soft`'s doc)
            // -> the same E=1944-length coded-bit-order slice `rate_match` selected
            // at TX time -> re-interleaved into wire/symbol order (matching
            // `eq_symbols`'/`llrs`' original order above).
            let punctured_len = nr_bg2::PUNCTURED_INFO_COLS * nr_bg2::ZC;
            let mother_only = &posterior[punctured_len..];
            let coded_posterior =
                rate_match::rate_match_llr(mother_only, k_used, CODED_BLOCK_LEN, 0);
            let interleaved_posterior = comp.interleaver.interleave_soft(&coded_posterior);

            let n_syms = interleaved_posterior.len() / bps;
            let mut soft_symbols = Vec::with_capacity(n_syms);
            let mut weights = Vec::with_capacity(n_syms);
            for i in 0..n_syms {
                let chunk = &interleaved_posterior[i * bps..(i + 1) * bps];
                let s = soft_symbol(comp.mapper.as_ref(), chunk);
                weights.push(s.norm_sqr());
                soft_symbols.push(s);
            }

            let (re_eq_symbols, re_noise_vars) = self
                .modem
                .reequalize_with_virtual_pilots(&soft_symbols, &weights);

            if !re_eq_symbols.is_empty() {
                // Re-demap exactly as step 3 did, against the re-equalized symbols.
                let mut llrs2 = Vec::with_capacity(coded_bits_needed);
                let fallback_nv2 = median_noise_variance(&re_noise_vars);
                for (i, &sym) in re_eq_symbols.iter().take(symbols_needed).enumerate() {
                    let nv = match re_noise_vars.get(i) {
                        Some(&v) if v > 1e-6 => v,
                        _ => fallback_nv2,
                    };
                    llrs2.extend(comp.mapper.demap_soft(sym, nv));
                }
                llrs2.truncate(coded_bits_needed);
                llrs2.resize(coded_bits_needed, 0.0);
                for llr in &mut llrs2 {
                    *llr = llr.clamp(-llr_clip, llr_clip);
                }

                // Re-deinterleave / re-dematch / re-pin (Task 3's exact pinning
                // mechanism, reused verbatim -- not reimplemented).
                let deinterleaved2 = comp.interleaver.deinterleave(&llrs2);
                let mut dematched2 = rate_match::rate_dematch(
                    &deinterleaved2,
                    k_used,
                    CODED_BLOCK_LEN,
                    0,
                    NrLdpc::MOTHER_LEN,
                );
                pin_known_pad(&mut dematched2, payload_bits, k_used, PIN);

                // Retry the decode exactly once.
                let (_, retry_bits, retry_converged, retry_iterations) =
                    self.ldpc.decode_soft_stats(&dematched2);

                iterations += retry_iterations;
                if retry_converged {
                    self.turbo_rescues.set(self.turbo_rescues.get() + 1);
                    decoded_bits = retry_bits;
                    converged = true;
                    noise_vars = re_noise_vars;
                }
            }
        }

        // Descramble to undo TX-side scrambling
        scramble(&mut decoded_bits);

        if !converged {
            return Err(ReceiveError::LdpcNotConverged { iterations });
        }

        // 7. Extract payload+CRC bytes (Phase 3 Task 1: `payload_len` application
        //    bytes followed by a `PAYLOAD_CRC_LEN`-byte CRC-32 trailer -- see
        //    `PAYLOAD_CRC32`'s doc).
        let payload_len = header.payload_len as usize;
        let total_len = payload_len + PAYLOAD_CRC_LEN;
        let mut payload_and_crc = Vec::with_capacity(total_len);
        for chunk in decoded_bits.chunks(8) {
            if chunk.len() == 8 && payload_and_crc.len() < total_len {
                let mut byte = 0u8;
                for (i, &bit) in chunk.iter().enumerate() {
                    byte |= (bit & 1) << (7 - i);
                }
                payload_and_crc.push(byte);
            }
        }

        // 8. Verify the CRC-32 trailer: LDPC convergence alone only means the
        //    decoder found *a* valid codeword, not that it's the *right* one --
        //    this catches the rare (but real, especially at low SNR) case where
        //    it converged to the wrong one.
        let (payload_bytes, crc_bytes) = payload_and_crc.split_at(payload_len);
        let received_crc = u32::from_be_bytes(
            crc_bytes
                .try_into()
                .expect("payload_and_crc always has exactly PAYLOAD_CRC_LEN trailer bytes"),
        );
        let expected_crc = PAYLOAD_CRC32.checksum(payload_bytes);
        if expected_crc != received_crc {
            return Err(ReceiveError::CrcMismatch);
        }
        let payload = payload_bytes.to_vec();

        // SNR estimate from the same per-carrier noise variances used for LLR scaling.
        let mean_nv = if noise_vars.is_empty() {
            1.0
        } else {
            noise_vars.iter().sum::<f32>() / noise_vars.len() as f32
        };
        let snr_db = 10.0 * (1.0 / mean_nv.max(1e-6)).log10();

        Ok((header, payload, snr_db))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_codec::ofdm::frame::CoppaFrameType;

    /// Independent brute-force reference: `sum_x x * P(x|LLRs)` over every
    /// `2^bits_per_symbol` constellation point, with `P(x|LLRs) =
    /// prod_i P(bit_i|LLRs[i])` computed directly from the sigmoid identity
    /// (NOT by calling `soft_symbol_generic`, so this is a genuinely
    /// independent check, not the production code checking itself). Per the
    /// task brief: "brute-force `sum_x x*P(x|LLRs)` over the constellation
    /// points ... removes all labeling ambiguity."
    fn brute_force_soft_symbol(mapper: &dyn ConstellationMapper, llrs: &[f32]) -> Complex32 {
        let bps = llrs.len();
        let mut acc = Complex32::new(0.0, 0.0);
        for idx in 0..(1usize << bps) {
            let bits: Vec<u8> = (0..bps)
                .map(|b| ((idx >> (bps - 1 - b)) & 1) as u8)
                .collect();
            let mut p = 1.0f32;
            for (i, &bit) in bits.iter().enumerate() {
                let p0 = 1.0 / (1.0 + (-llrs[i]).exp());
                p *= if bit == 0 { p0 } else { 1.0 - p0 };
            }
            acc += mapper.map(&bits) * p;
        }
        acc
    }

    /// A handful of varied (non-trivial, non-symmetric) LLR fixtures -- not just
    /// all-zero/all-huge -- so the brute-force check actually exercises the
    /// closed forms' cross terms, not just their behavior at symmetric points
    /// where many terms coincidentally vanish.
    fn llr_fixtures(n: usize) -> Vec<Vec<f32>> {
        let raw: [f32; 8] = [2.7, -1.3, 0.4, -3.8, 5.1, -0.2, 1.9, -6.4];
        (0..4)
            .map(|shift| raw.iter().cycle().skip(shift).take(n).copied().collect())
            .collect()
    }

    #[test]
    fn soft_symbol_bpsk_matches_brute_force() {
        let mapper = coppa_codec::bpsk::BpskMapper;
        for llrs in llr_fixtures(1) {
            let closed = soft_symbol_bpsk(llrs[0]);
            let brute = brute_force_soft_symbol(&mapper, &llrs);
            assert!(
                (closed - brute).norm() < 1e-5,
                "llrs={llrs:?}: closed={closed:?} brute={brute:?}"
            );
        }
    }

    #[test]
    fn soft_symbol_qpsk_matches_brute_force() {
        let mapper = coppa_codec::qpsk::QpskMapper;
        for llrs in llr_fixtures(2) {
            let closed = soft_symbol_qpsk(llrs[0], llrs[1]);
            let brute = brute_force_soft_symbol(&mapper, &llrs);
            assert!(
                (closed - brute).norm() < 1e-5,
                "llrs={llrs:?}: closed={closed:?} brute={brute:?}"
            );
        }
    }

    #[test]
    fn soft_pam4_matches_brute_force_16qam_per_axis() {
        // Brute force over the full 16-point 2D constellation, split per axis:
        // re depends only on bits[0..2], im only on bits[2..4] (Qam16Mapper's
        // `map`), so a 4-point per-axis brute force (fixing the OTHER axis's
        // bits, which don't affect this axis' marginal) is equivalent to (and
        // cheaper than) enumerating all 16 -- but to genuinely avoid assuming
        // that independence structure in the *test*, brute-force the full
        // 4-LLR/16-point joint expectation and compare against the closed-form
        // combination directly.
        let mapper = coppa_codec::qam16::Qam16Mapper;
        for llrs in llr_fixtures(4) {
            let closed = Complex32::new(soft_pam4(llrs[0], llrs[1]), soft_pam4(llrs[2], llrs[3]));
            let brute = brute_force_soft_symbol(&mapper, &llrs);
            assert!(
                (closed - brute).norm() < 1e-5,
                "llrs={llrs:?}: closed={closed:?} brute={brute:?}"
            );
        }
    }

    #[test]
    fn soft_qam64_axis_matches_brute_force_64qam_per_axis() {
        let mapper = coppa_codec::qam64::Qam64Mapper;
        for llrs in llr_fixtures(6) {
            let closed = Complex32::new(
                soft_qam64_axis(llrs[0], llrs[1], llrs[2]),
                soft_qam64_axis(llrs[3], llrs[4], llrs[5]),
            );
            let brute = brute_force_soft_symbol(&mapper, &llrs);
            assert!(
                (closed - brute).norm() < 1e-5,
                "llrs={llrs:?}: closed={closed:?} brute={brute:?}"
            );
        }
    }

    #[test]
    fn soft_symbol_generic_8psk_matches_brute_force() {
        let mapper = coppa_codec::psk8::Psk8Mapper;
        for llrs in llr_fixtures(3) {
            let generic = soft_symbol_generic(&mapper, &llrs);
            let brute = brute_force_soft_symbol(&mapper, &llrs);
            assert!(
                (generic - brute).norm() < 1e-5,
                "llrs={llrs:?}: generic={generic:?} brute={brute:?}"
            );
        }
    }

    #[test]
    fn soft_symbol_dispatch_matches_bits_per_symbol() {
        // Sanity check the bits_per_symbol -> modulation dispatch in `soft_symbol`
        // agrees with each mapper's own bits_per_symbol for every currently
        // defined speed level (guards the "bits_per_symbol uniquely identifies
        // modulation in this ladder" assumption `soft_symbol`'s doc relies on).
        for level in [1u8, 2, 3, 4, 5, 6, 7, 9, 10] {
            let (mapper, _) = crate::modem::speed_levels::speed_level_components(level).unwrap();
            let bps = mapper.bits_per_symbol();
            let llrs = vec![1.0f32; bps];
            // Must not panic and must produce a finite symbol.
            let s = soft_symbol(mapper.as_ref(), &llrs);
            assert!(
                s.re.is_finite() && s.im.is_finite(),
                "level {level}: non-finite {s:?}"
            );
        }
    }

    /// Regression test: `with_turbo(false)` reproduces the exact pre-Task-5
    /// single-decode-attempt behavior on a clean channel (no first-pass failure
    /// to retry, so turbo on/off should be indistinguishable here) -- and
    /// `turbo_attempts()` must stay at 0 when nothing ever fails.
    #[test]
    fn turbo_off_still_decodes_cleanly_and_never_fires() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1).with_turbo(false);
        assert!(!tx.turbo());
        let payload = vec![0x5Au8; 20];
        let header = make_header(2, payload.len() as u16);
        let samples = tx
            .transmit(&header, &payload)
            .expect("payload within this test's speed level capacity");
        let (_h, rx) = tx.receive(&samples).expect("clean loopback should decode");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
        assert_eq!(tx.turbo_attempts(), 0);
    }

    #[test]
    fn turbo_on_still_decodes_cleanly_on_clean_channel() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        assert!(tx.turbo());
        let payload = vec![0x5Au8; 20];
        let header = make_header(2, payload.len() as u16);
        let samples = tx
            .transmit(&header, &payload)
            .expect("payload within this test's speed level capacity");
        let (_h, rx) = tx.receive(&samples).expect("clean loopback should decode");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    /// Step 1 (statistical, run manually in Step 3): 200 poor-channel seeds at
    /// the pre-Task-5 ~30% FER operating point (measured directly below, not
    /// assumed) -- acceptance: turbo-on FER <= 0.75x turbo-off FER.
    ///
    /// Run: `cargo test -p coppa-protocol --lib -- --ignored --nocapture \
    /// turbo_reestimation_reduces_fer_on_poor_channel`
    #[test]
    #[ignore = "statistical (200 seeds x 2 configs); run manually, see doc comment"]
    fn turbo_reestimation_reduces_fer_on_poor_channel() {
        use coppa_channel::watterson::WattersonPreset;

        const LEVEL: u8 = 2; // BPSK 1/2
        const PAYLOAD_BYTES: usize = 121; // level 2's full-codeword payload (MODES table)
                                          // Measured directly (separate probe, not assumed) as the pre-Task-5
                                          // ~30% FER operating point on hf_standard/watterson-poor for level 2.
        const SNR_DB: f32 = 9.0;
        const SEEDS: u64 = 200;

        let run = |turbo: bool| -> usize {
            let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1).with_turbo(turbo);
            let header = make_header(LEVEL, PAYLOAD_BYTES as u16);
            let mut failures = 0usize;
            for trial in 0..SEEDS {
                let seed = 0x7075_0000u64.wrapping_add(trial);
                use rand::rngs::StdRng;
                use rand::{RngExt, SeedableRng};
                let mut rng = StdRng::seed_from_u64(seed);
                let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|_| rng.random::<u8>()).collect();
                let clean = tx
                    .transmit(&header, &payload)
                    .expect("payload within this test's speed level capacity");
                let faded = coppa_channel::watterson::watterson(
                    &clean,
                    48_000.0,
                    &WattersonPreset::Poor.config(),
                    seed ^ 0x3333_3333_3333_3333,
                );
                let noisy =
                    coppa_channel::awgn_seeded(&faded, SNR_DB, seed ^ 0x5555_5555_5555_5555);
                let ok =
                    matches!(tx.receive(&noisy), Ok((_, rx)) if rx[..payload.len()] == payload[..]);
                if !ok {
                    failures += 1;
                }
            }
            failures
        };

        let off_failures = run(false);
        let on_failures = run(true);
        let off_fer = off_failures as f64 / SEEDS as f64;
        let on_fer = on_failures as f64 / SEEDS as f64;
        println!(
            "turbo-off: {off_failures}/{SEEDS} (FER={off_fer:.3}); turbo-on: {on_failures}/{SEEDS} (FER={on_fer:.3})"
        );
        assert!(
            (on_failures as f64) <= 0.75 * (off_failures as f64),
            "turbo-on FER ({on_fer:.3}) should be <= 0.75x turbo-off FER ({off_fer:.3}): \
             on={on_failures} off={off_failures}"
        );
    }

    /// Regression test: VHF-routed speed levels (5,6,7,9,10 via `select_profile` in
    /// coppa-bench, which chooses `vhf_wide` for level >= 5) previously fell back to
    /// an unconditioned TX path that never leveled the preamble against the much
    /// quieter header/payload body, leaving the transmitted peak above full scale
    /// (measured ~1.026 before the fix) and the payload badly underpowered relative
    /// to the whole-frame mean power any AWGN budget is referenced to. Exercise many
    /// random payloads through the full `CoppaTransceiver` (LDPC + interleave +
    /// mapping) at a VHF speed level with zero channel impairment.
    #[test]
    fn vhf_level5_transceiver_round_trips_with_bounded_peak() {
        use coppa_codec::ofdm::CoppaProfile;
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};
        let profile = CoppaProfile::vhf_wide();
        let tx = CoppaTransceiver::new(profile, 1);
        let mut ok_count = 0;
        for trial in 0..20u64 {
            let seed = 0xABCDu64.wrapping_add(trial);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload_bytes = 130usize; // level 5 payload size per bench MODES
            let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
            let header = CoppaHeader {
                version: 1,
                phy_mode: 0,
                frame_type: CoppaFrameType::Data,
                bandwidth: 1,
                fec_type: 0,
                speed_level: 5,
                seq_num: 0,
                payload_len: payload_bytes as u16,
            };
            let clean = tx
                .transmit(&header, &payload)
                .expect("payload within this test's speed level capacity");
            let peak = clean.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
            assert!(
                peak <= 0.5001,
                "trial {trial}: TX peak must be normalized to ~0.5 FS, got {peak}"
            );
            let result = tx.receive(&clean);
            if result.is_ok() {
                ok_count += 1;
            }
        }
        assert!(
            ok_count == 20,
            "all 20 clean-channel VHF trials should decode, got {ok_count}/20"
        );
    }

    fn make_header(speed_level: u8, payload_len: u16) -> CoppaHeader {
        CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level,
            seq_num: 0,
            payload_len,
        }
    }

    /// Phase 3 Task 1(a): a completely valid, well-formed frame (LDPC converges
    /// cleanly) whose payload trailer is a deliberately WRONG CRC-32 must be
    /// rejected by `receive` with `Err(ReceiveError::CrcMismatch)`.
    ///
    /// Per the task brief: reliably corrupting a converged LDPC decode to the
    /// *wrong* codeword via LLR flips isn't reliably constructible (that's the
    /// rare failure mode this whole feature exists to catch, not something to
    /// engineer on demand in a unit test). Instead this tests the CRC path
    /// directly by replicating `transmit`'s exact pipeline (same pattern as
    /// `known_pad_prbs_matches_scrambled_pad_ground_truth` above) but appending a
    /// wrong CRC-32 trailer instead of the real checksum over `payload` --
    /// everything downstream (LDPC encode, rate-match, interleave, map,
    /// modulate) is untouched production code, so this exercises the real
    /// decode + CRC-check path on RX, not a mock.
    #[test]
    fn receive_rejects_wrong_crc() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let comp = tx.codecs.get(&2).expect("level 2 must exist"); // BPSK 1/2
        let k_used = comp.k_used;
        let payload = vec![0x11u8; 20];
        let header = make_header(2, payload.len() as u16);

        let real_crc = PAYLOAD_CRC32.checksum(&payload);
        let wrong_crc = real_crc ^ 0xFFFF_FFFF; // definitely different
        assert_ne!(real_crc, wrong_crc);

        let mut payload_with_bad_crc = payload.clone();
        payload_with_bad_crc.extend_from_slice(&wrong_crc.to_be_bytes());

        let mut info_bits = Vec::with_capacity(NrLdpc::INFO_LEN);
        for &byte in &payload_with_bad_crc {
            for shift in (0..8).rev() {
                info_bits.push((byte >> shift) & 1);
            }
        }
        info_bits.resize(k_used, 0u8);
        info_bits.resize(NrLdpc::INFO_LEN, 0u8);
        scramble(&mut info_bits);

        let mother = tx.ldpc.encode(&info_bits);
        let coded_bits = rate_match::rate_match(&mother, k_used, CODED_BLOCK_LEN, 0);
        let interleaved = comp.interleaver.interleave(&coded_bits);
        let symbols = comp.mapper.map_bits(&interleaved);
        let sl = SPEED_LEVELS.iter().find(|s| s.level == 2).unwrap();
        let samples = tx
            .modem
            .modulate_mapped(&header, &symbols, sl.papr_target_db);

        let result = tx.receive(&samples);
        assert!(
            matches!(result, Err(ReceiveError::CrcMismatch)),
            "expected Err(CrcMismatch) for a wrong CRC-32 trailer, got {result:?}"
        );
    }

    /// Phase 3 Task 1(b): `transmit` with `payload.len() > max_payload(level)`
    /// must return `Err(TransmitError::PayloadTooLarge { max })` instead of the
    /// pre-Task-1 codec's silent truncation.
    #[test]
    fn transmit_rejects_oversize_payload() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let level = 2u8; // BPSK 1/2
        let max = max_payload_for_level(level).expect("level 2 must exist");
        let payload = vec![0u8; max + 1];
        let header = make_header(level, payload.len() as u16);

        let result = tx.transmit(&header, &payload);
        assert_eq!(
            result,
            Err(TransmitError::PayloadTooLarge { max }),
            "expected Err(PayloadTooLarge {{ max: {max} }}) for an oversized payload"
        );
    }

    /// Phase 3 Task 1(c): a payload at exactly this level's max capacity must
    /// still transmit and round-trip cleanly (the oversize check is a strict
    /// `>`, not `>=`).
    #[test]
    fn loopback_exact_max_payload_succeeds() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let level = 2u8; // BPSK 1/2
        let max = max_payload_for_level(level).expect("level 2 must exist");
        let payload = vec![0x7Eu8; max];
        let header = make_header(level, payload.len() as u16);

        let samples = tx
            .transmit(&header, &payload)
            .expect("exact-max payload should transmit");
        let (_h, rx) = tx
            .receive(&samples)
            .expect("exact-max payload should round-trip");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    #[test]
    fn loopback_survives_ssb_filter_and_50hz_mistune() {
        // The bar Phase 1 exists to clear: a real radio's passband + a realistic mistune.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0xA7u8; 100];
        let header = make_header(2, payload.len() as u16);
        let s = tx
            .transmit(&header, &payload)
            .expect("payload within this test's speed level capacity");
        let through_rig = coppa_channel::ssb_filter(&s, 48_000.0);
        let mistuned = coppa_channel::frequency_shift(&through_rig, 47.0, 48_000.0);
        let (_h, rx) = tx
            .receive(&mistuned)
            .expect("must decode through SSB filter + 47 Hz CFO");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    #[test]
    fn loopback_survives_ssb_filter_only() {
        // Same as above but with 0 Hz mistune: this is the part of the bar this task must
        // clear now (CFO correction on this mistuned path lands in Task 6).
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0xA7u8; 100];
        let header = make_header(2, payload.len() as u16);
        let s = tx
            .transmit(&header, &payload)
            .expect("payload within this test's speed level capacity");
        let through_rig = coppa_channel::ssb_filter(&s, 48_000.0);
        let untuned = coppa_channel::frequency_shift(&through_rig, 0.0, 48_000.0);
        let (_h, rx) = tx
            .receive(&untuned)
            .expect("must decode through SSB filter alone (no mistune)");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_cfo_correction() {
        // An 8 Hz CFO collapses the link without correction; the RX must estimate + remove it.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = b"CFO correction works";
        let header = make_header(2, payload.len() as u16);
        let samples = tx
            .transmit(&header, payload)
            .expect("payload within this test's speed level capacity");
        let injected = coppa_codec::ofdm::sync::remove_cfo(&samples, -8.0, 48_000.0); // +8 Hz
        let (_h, rx) = tx
            .receive(&injected)
            .expect("should recover after CFO correction");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    /// Regression test for the sync timing-anchor fix in
    /// `docs/adr/004-strongest-path-timing.md`: `hf_standard`'s sparse (4-)pilot
    /// protected header must survive Watterson-Moderate fading at a level (1,
    /// BPSK 1/4) and SNR (21 dB) that pre-Phase-1 measurements
    /// (`results/rebaseline-2026-07/moderate.csv`) clear comfortably. The bug this
    /// guards against (`SyncDetector` anchoring on a weak-but-earliest multipath
    /// tap instead of the strongest one) floored this exact scenario at a ~65-70%
    /// success rate regardless of SNR; the 80% bar is comfortably below normal
    /// trial-to-trial variance at this operating point and well above that floor.
    #[test]
    fn hf_standard_header_survives_watterson_moderate_fading() {
        use coppa_channel::watterson::WattersonPreset;

        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0x5Au8; 20];
        let header = make_header(1, payload.len() as u16); // level 1 = BPSK 1/4
        let clean = tx
            .transmit(&header, &payload)
            .expect("payload within this test's speed level capacity");

        const TRIALS: u64 = 30;
        let mut ok = 0u64;
        for trial in 0..TRIALS {
            let seed = 0xFADE_0000u64.wrapping_add(trial);
            let faded = coppa_channel::watterson::watterson(
                &clean,
                48_000.0,
                &WattersonPreset::Moderate.config(),
                seed,
            );
            let noisy = coppa_channel::awgn_seeded(&faded, 21.0, seed ^ 0x55AA);
            if matches!(tx.receive(&noisy), Ok((_, rx)) if rx[..payload.len()] == payload[..]) {
                ok += 1;
            }
        }
        assert!(
            ok * 100 >= TRIALS * 80,
            "hf_standard level-1 header should survive Watterson-Moderate fading at 21 dB in \
             the large majority of trials, got {ok}/{TRIALS} -- if this regresses, check the \
             sync timing anchor policy (docs/adr/004-strongest-path-timing.md)"
        );
    }

    #[test]
    fn test_transceiver_bpsk_rate_half_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = b"Hello Phase C!";
        let header = make_header(2, payload.len() as u16);

        let samples = tx
            .transmit(&header, payload)
            .expect("payload within this test's speed level capacity");
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, header.speed_level);
        assert_eq!(rx_header.payload_len, header.payload_len);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_qpsk_rate_half_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        let header = make_header(3, payload.len() as u16);

        let samples = tx
            .transmit(&header, payload.as_slice())
            .expect("payload within this test's speed level capacity");
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, 3);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_16qam_rate_1_2_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0x42u8; 100];
        let header = make_header(6, payload.len() as u16);

        let samples = tx
            .transmit(&header, payload.as_slice())
            .expect("payload within this test's speed level capacity");
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, 6);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_16qam_survives_flat_gain() {
        // A flat channel gain (0.5) shrinks the constellation; MMSE leaves the equalized symbols
        // at the wrong amplitude and 16QAM mis-decodes. Gain-normalization (Y/H) must restore it.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = vec![0x3Cu8; 40];
        let header = make_header(6, payload.len() as u16); // 16QAM 1/2
        let mut samples = tx
            .transmit(&header, payload.as_slice())
            .expect("payload within this test's speed level capacity");
        for s in samples.iter_mut() {
            *s *= 0.5; // flat channel gain
        }
        let (_h, rx) = tx
            .receive(&samples)
            .expect("16QAM should survive a flat 0.5 gain");
        assert_eq!(&rx[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_hf_robust_bpsk_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = b"Hello robust HF profile!";
        let header = make_header(2, payload.len() as u16); // BPSK 1/2
        let samples = tx
            .transmit(&header, payload)
            .expect("payload within this test's speed level capacity");
        let (rx_header, rx_payload) = tx
            .receive(&samples)
            .expect("hf_robust decode should succeed");
        assert_eq!(rx_header.speed_level, 2);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_hf_robust_qpsk_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = vec![0x5Au8; 60];
        let header = make_header(3, payload.len() as u16); // QPSK 1/2
        let samples = tx
            .transmit(&header, payload.as_slice())
            .expect("payload within this test's speed level capacity");
        let (rx_header, rx_payload) = tx
            .receive(&samples)
            .expect("hf_robust QPSK decode should succeed");
        assert_eq!(rx_header.speed_level, 3);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_header_survives_bit_errors_in_header_region() {
        // A few sign flips in the header OFDM symbols used to corrupt the frame
        // (unprotected header). With Golay+CRC protection the header must recover.
        let tx = CoppaTransceiver::new(CoppaProfile::hf_robust(), 1);
        let payload = vec![0x5Au8; 40];
        let header = make_header(2, payload.len() as u16); // BPSK 1/2
        let mut samples = tx
            .transmit(&header, payload.as_slice())
            .expect("payload within this test's speed level capacity");
        // Perturb a handful of samples inside the header region (after preamble +
        // fine-sync = 3 symbols). One robust symbol = fft_size + cp = 1260 samples.
        let sym = 1260;
        let header_start = 3 * sym;
        for i in 0..8 {
            let idx = header_start + i * 37;
            if idx < samples.len() {
                samples[idx] += 0.15; // small additive perturbation
            }
        }
        let (rx_header, rx_payload) = tx
            .receive(&samples)
            .expect("protected header should recover from small perturbations");
        assert_eq!(rx_header.speed_level, 2);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    /// Step 1(c): the pinned positions computed in `receive` must equal the
    /// pad's actual scrambled (transmitted) value, for every position beyond
    /// the payload+CRC. Replicates `transmit`'s exact `info_bits` construction
    /// (real payload bits + CRC-32 trailer bits (Phase 3 Task 1) + zero pad out
    /// to k_used + zero pad out to the fixed NR BG2 mother-code info width, whole
    /// vector scrambled) as ground truth, then checks it against
    /// `prbs_bits(NrLdpc::INFO_LEN)` at the same indices -- this is exactly what
    /// `receive`'s `pin_known_pad` call relies on (Task 3, extended for the
    /// mother code in Task 4, extended again for the CRC trailer in Phase 3
    /// Task 1).
    #[test]
    fn known_pad_prbs_matches_scrambled_pad_ground_truth() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let comp = tx.codecs.get(&2).expect("level 2 must exist"); // BPSK 1/2
        let k_used = comp.k_used;
        let payload_bytes = 20usize;
        // Real (non-pad) bits now cover payload + CRC-32 trailer -- see
        // `PAYLOAD_CRC32`'s doc.
        let real_bits_count = (payload_bytes + PAYLOAD_CRC_LEN) * 8;
        assert!(
            real_bits_count < k_used,
            "test assumes real padding exists within k_used"
        );

        // Ground truth: exactly replicate `transmit`'s info_bits construction.
        let payload = vec![0xA5u8; payload_bytes];
        let checksum = PAYLOAD_CRC32.checksum(&payload);
        let mut payload_with_crc = Vec::with_capacity(payload.len() + PAYLOAD_CRC_LEN);
        payload_with_crc.extend_from_slice(&payload);
        payload_with_crc.extend_from_slice(&checksum.to_be_bytes());

        let mut info_bits = Vec::with_capacity(NrLdpc::INFO_LEN);
        for &byte in &payload_with_crc {
            for shift in (0..8).rev() {
                info_bits.push((byte >> shift) & 1);
            }
        }
        info_bits.resize(k_used, 0u8);
        info_bits.resize(NrLdpc::INFO_LEN, 0u8);
        scramble(&mut info_bits);

        let pad_prbs = crate::fec::scrambler::prbs_bits(NrLdpc::INFO_LEN);
        assert_eq!(
            &info_bits[real_bits_count..],
            &pad_prbs[real_bits_count..],
            "prbs_bits(NrLdpc::INFO_LEN)'s pad-region tail must match transmit()'s actual \
             scrambled pad -- this is the ground truth `receive`'s pinning relies on"
        );
    }

    /// Step 1(b): statistical integration test for known-pad LLR pinning (Task 3).
    ///
    /// `CoppaTransceiver::receive`'s full OFDM pipeline can't demonstrate this
    /// directly: a dedicated bench sweep (`coppa-bench`'s `task3_short_payload_gate`
    /// example; see the Task 3 report for the full before/after CSVs) found that
    /// for a 20-byte payload at level 2 on `hf_standard`/AWGN, *every* frame
    /// failure across the whole relevant SNR range is a sync/header failure, not
    /// an LDPC non-convergence -- confirmed by direct instrumentation showing the
    /// LDPC decode converges 100% of the time whenever sync succeeds, identically
    /// whether or not pad bits are pinned. OFDM sync is strictly the binding
    /// constraint here, so the pinning's effect on the LDPC margin is invisible
    /// end-to-end.
    ///
    /// To test the actual mechanism (not masked by sync), this replicates the
    /// exact code path `receive`/`transmit` use for the FEC layer -- `scramble`,
    /// `prbs_bits`, `LdpcCodec`, and `BpskMapper`'s (now-fixed) exact max-log LLR
    /// scale -- but maps coded bits directly to BPSK symbols and adds AWGN,
    /// bypassing OFDM/sync entirely. This is exactly the isolated measurement
    /// `coppa-bench`'s `task3_fec_isolated_gate` example performs; see the Task 3
    /// report for the full sweep. That sweep found: no-pin FER<=10% threshold =
    /// 2.0 dB, pinned threshold = -1.0 dB (a 3.0 dB shift, matching the brief's
    /// expected 1.5-3 dB). This test fixes the SNR at 1.5 dB below the no-pinning
    /// threshold (0.5 dB) and asserts pinning recovers the large majority of
    /// frames there (measured 393/400 = 98.25% in the full sweep at this exact
    /// point; 100 seeds here for a quick but still statistically meaningful
    /// check).
    #[test]
    #[ignore = "statistical (100 seeds); run manually: cargo test -p coppa-protocol --lib -- --ignored known_pad_pinning_recovers_below_no_pinning_threshold"]
    fn known_pad_pinning_recovers_below_no_pinning_threshold() {
        use crate::fec::ldpc::{CodeRate, LdpcCodec};
        use crate::fec::scrambler::prbs_bits;
        use coppa_codec::bpsk::BpskMapper;
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};

        const PAYLOAD_BYTES: usize = 20;
        const PIN: f32 = 64.0;
        const LLR_CLIP: f32 = 20.0;
        // Measured no-pinning FER<=10% threshold (task3_fec_isolated_gate) is 2.0 dB;
        // 1.5 dB below that is 0.5 dB.
        const TEST_SNR_DB: f32 = 0.5;
        const SEEDS: u64 = 100;

        let codec = LdpcCodec::new(CodeRate::Rate1_2); // level 2: BPSK 1/2, 972 info bits
        let info_bits = codec.code().info_bits();
        let payload_bits_count = PAYLOAD_BYTES * 8;
        let mapper = BpskMapper;

        let mut successes = 0u64;
        for trial in 0..SEEDS {
            let seed = 0x9EED_0000u64.wrapping_add(trial);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload: Vec<u8> = (0..PAYLOAD_BYTES).map(|_| rng.random::<u8>()).collect();

            let mut info: Vec<u8> = Vec::with_capacity(info_bits);
            for &byte in &payload {
                for shift in (0..8).rev() {
                    info.push((byte >> shift) & 1);
                }
            }
            info.resize(info_bits, 0u8);
            scramble(&mut info);
            let coded = codec.encode(&info);

            let clean: Vec<f32> = coded.iter().map(|&b| mapper.map(&[b]).re).collect();
            let noisy =
                coppa_channel::awgn_seeded(&clean, TEST_SNR_DB, seed ^ 0x5A5A_5A5A_5A5A_5A5Au64);
            let nv = 10f32.powf(-TEST_SNR_DB / 10.0);
            let mut llrs: Vec<f32> = noisy
                .iter()
                .map(|&re| (4.0 * re / nv).clamp(-LLR_CLIP, LLR_CLIP))
                .collect();

            let pad_prbs = prbs_bits(info_bits);
            for (i, &prbs_bit) in pad_prbs
                .iter()
                .enumerate()
                .take(info_bits)
                .skip(payload_bits_count)
            {
                llrs[i] = if prbs_bit == 0 { PIN } else { -PIN };
            }

            let (mut decoded, converged) = codec.decode_checked(&llrs);
            if !converged {
                continue;
            }
            scramble(&mut decoded);

            let mut out = Vec::with_capacity(PAYLOAD_BYTES);
            for chunk in decoded.chunks(8) {
                if chunk.len() == 8 && out.len() < PAYLOAD_BYTES {
                    let mut byte = 0u8;
                    for (i, &bit) in chunk.iter().enumerate() {
                        byte |= (bit & 1) << (7 - i);
                    }
                    out.push(byte);
                }
            }
            if out == payload {
                successes += 1;
            }
        }

        assert!(
            successes * 100 >= SEEDS * 90,
            "known-pad pinning should recover the large majority of frames 1.5 dB below \
             the no-pinning FER<=10% threshold, got {successes}/{SEEDS}"
        );
    }
}
