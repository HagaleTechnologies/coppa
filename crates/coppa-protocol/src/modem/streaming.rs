//! Streaming frame receiver: feed audio in arbitrary-sized chunks, get back zero or
//! more fully-decoded frames.
//!
//! Replaces the batch-only `CoppaTransceiver::receive` (which needs its caller to
//! already know where a whole frame lives in a buffer) with a state machine that
//! owns a ring of raw samples plus a [`SyncDetector`], and only pays for a full
//! LDPC/interleave/demap pass once per completed (or provably-failed) candidate —
//! not once per pushed chunk. That "once per candidate, not once per chunk" framing
//! is a statement about amortized throughput across a session, not about the
//! latency of any one [`StreamingReceiver::push_samples`] call: the specific call
//! that completes a candidate pays that full demod/FEC cost synchronously before
//! returning (see that method's doc).
//!
//! State machine (locked, see `docs/superpowers/plans/2026-07-03-phase1-radio-reality.md`
//! Task 7): `Searching` (represented here as `pending: None`) feeds every pushed
//! sample to the `SyncDetector` and queues whatever candidates it confirms.
//! `Accumulating { start, need }` (represented as `pending: Some(Pending { .. })`)
//! buffers samples from a candidate's `frame_start` until `need` are available, in
//! two stages:
//!
//! 1. First `need = rx_group_delay + (3 + header_syms) * symbol_len` — just enough
//!    for the preamble, probe/fine-sync symbol, and the protected header, PLUS the
//!    RX bandpass filter's own group delay (see [`StreamingReceiver::header_peek`]'s
//!    doc for why that's needed). Once buffered, the header ALONE is demodulated
//!    (`CoppaTransceiver::demodulate_header`) to learn the frame's speed level. A
//!    header decode failure discards the candidate and
//!    resumes searching AT `start + symbol_len` (not after the whole buffered
//!    header window) — the `SyncDetector`'s own state is untouched by this and
//!    keeps running on every sample pushed regardless, so a second frame's preamble
//!    may already be queued by the time the first candidate is abandoned.
//! 2. Once the header's speed level is known, `need` is extended to cover the full
//!    frame (payload symbol count from `SPEED_LEVELS`, scaled by the header's
//!    `codewords` count — COP-69: the streaming path previously omitted that
//!    multiplier and so could not decode a multi-codeword frame at all). Once
//!    buffered, the full `CoppaTransceiver::receive_with_metrics` runs on exactly
//!    that slice, is emitted as a [`DecodedFrame`], and the receiver returns to
//!    `Searching`.
use std::collections::VecDeque;

use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::ofdm::frame::CoppaHeader;
use coppa_codec::ofdm::header_fec;
use coppa_codec::ofdm::sync_detector::{SyncCandidate, SyncDetector};
use coppa_codec::ofdm::CoppaProfile;

use crate::modem::transceiver::{CoppaTransceiver, ReceiveError, MAX_CODEWORDS};

/// LDPC coded block length (Z=81, 24 base columns) — constant for all code rates;
/// mirrors `CoppaModem::demodulate_frame`'s own `CODED_BLOCK_LEN`.
const CODED_BLOCK_LEN: usize = 1944;

/// Tap count of the RX bandpass filter `CoppaTransceiver`/`CoppaModem` use for HF
/// profiles (`phy_mode == 0`) — mirrors the constant baked into both of those
/// (`coppa_dsp::fir::design_bandpass(601, ..)`). Needed here only to compute that
/// filter's group delay; see [`StreamingReceiver::header_peek`]'s doc.
const RX_BPF_TAPS: usize = 601;

/// Total OFDM symbols one frame occupies on air: 3 preamble symbols, the
/// protected header, and a payload region of `codewords` back-to-back
/// 1944-coded-bit LDPC blocks at this speed level's constellation size, packed
/// `data_carriers`-at-a-time into OFDM symbols.
///
/// COP-69: `codewords` comes from the decoded header (`header.codewords`,
/// 1..=[`MAX_CODEWORDS`]). This is the same multiplier the batch demod
/// (`CoppaModem::demodulate_frame_impl`) applies; the streaming path must agree
/// with it or it under-buffers — and then mis-slices — every multi-codeword
/// frame. Phase 5 prep: a short-ACK frame class will extend this helper with a
/// frame-class parameter instead of always assuming a full codeword-block
/// payload region.
fn frame_symbol_count(
    header_syms: usize,
    bits_per_symbol: usize,
    data_carriers: usize,
    codewords: usize,
) -> usize {
    let coded_symbols = CODED_BLOCK_LEN.div_ceil(bits_per_symbol) * codewords.max(1);
    let payload_syms = coded_symbols.div_ceil(data_carriers);
    3 + header_syms + payload_syms
}

/// One frame fully decoded by [`StreamingReceiver::push_samples`].
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub header: CoppaHeader,
    pub payload: Vec<u8>,
    /// `10*log10(1 / mean(per-carrier noise variance))`, from
    /// `CoppaTransceiver::receive_with_metrics`.
    pub snr_db: f32,
    /// Per-frame recommended speed level (`coppa_ml::recommend_speed_level`), from
    /// `CoppaTransceiver::receive_with_metrics`'s 4th element -- fed back to the
    /// remote sender on an outgoing ACK (`TransportPdu::new_ack_with_rate`), which
    /// applies it via `coppa_ml::RateLoop`.
    pub recommended_level: u8,
    /// Two-stage Moose CFO estimate (Hz) captured when this frame's candidate was
    /// first confirmed by the `SyncDetector` (see `sync_detector` module docs).
    pub cfo_hz: f32,
    /// Absolute sample index (in the coordinate system of every sample ever pushed
    /// to this receiver) of the frame's preamble start.
    pub frame_start: u64,
    /// This frame's measured multipath delay spread (ms), from
    /// `CoppaTransceiver::receive_with_metrics`'s 5th element (originally
    /// `DelayDomainEstimator::delay_spread_ms`, Task 6b). Feeds `coppa_ml::CpGate`'s
    /// daemon wiring -- see `docs/superpowers/specs/
    /// 2026-07-25-cpgate-daemon-wiring-design.md`.
    pub delay_spread_ms: f32,
}

/// An in-progress candidate being accumulated toward a full frame.
struct Pending {
    /// Absolute sample index of this candidate's preamble start.
    start: u64,
    /// Total samples needed (from `start`) before the next processing step can run.
    /// Starts at the header-only requirement; extended once the header is known.
    need: usize,
    /// CFO estimate captured when this candidate was confirmed.
    cfo_hz: f32,
    /// Set once the header-only peek has succeeded.
    header: Option<CoppaHeader>,
}

/// Streaming frame receiver: owns a `SyncDetector` + ring of raw samples plus a
/// `CoppaTransceiver` for the full demod/FEC pass. See module docs for the state
/// machine.
pub struct StreamingReceiver {
    symbol_len: usize,
    /// This profile's data (non-pilot) carriers per OFDM symbol — the same value
    /// `CoppaModem::data_carriers_per_symbol` computes internally (see
    /// `CoppaProfile::data_carriers`'s doc for why the two coincide).
    data_carriers: usize,
    /// Header region length in OFDM symbols (protected-header coded bits divided by
    /// this profile's data carriers per symbol) — constant for a given profile,
    /// independent of speed level.
    header_syms: usize,
    /// Upper bound on any single frame's sample length for this profile (worst
    /// case across all 9 speed levels at up to [`MAX_CODEWORDS`] codewords);
    /// used to keep the ring bounded while searching.
    max_frame_samples: usize,
    /// Group delay (samples) of the RX bandpass filter `CoppaTransceiver` applies
    /// internally for HF profiles; `0` for VHF (no RX bandpass at all). See
    /// [`Self::header_peek`]'s doc for why this receiver needs to know it.
    rx_group_delay: usize,
    sync: SyncDetector,
    transceiver: CoppaTransceiver,
    /// Raw (unfiltered) samples, retained from `ring_base` (absolute index of
    /// `ring[0]`) onward, in real-world sample-index coordinates — the same
    /// coordinate system `total_pushed`, candidate `frame_start`s, `resume_from`,
    /// and `Pending::start` all use.
    ///
    /// This receiver does NOT run any continuous filter over the incoming stream:
    /// an earlier version of this code did (to keep `SyncDetector` and the ring in
    /// a consistent "filtered" domain matching `CoppaTransceiver::receive`'s own
    /// internal detection), but measured directly (see the Task 7 report), a
    /// continuous 601-tap HF RX-bandpass filter over 10 s of audio cost ~13x more
    /// than a bare `SyncDetector` pass (0.020x realtime vs. the 0.005x target) —
    /// disproportionate to the small fraction of incoming audio that's ever inside
    /// an actual candidate window. Instead:
    /// - `SyncDetector` runs on raw samples too. This gives up some detection
    ///   margin on a genuinely noisy channel relative to filtering first (the
    ///   `sync_detector` module's own tests measure ~9 dB) — a real, deliberate,
    ///   documented trade-off, not a correctness bug: it affects miss rate on a
    ///   weak real signal, not the false-positive rejection of noise/tones (which
    ///   `SyncDetector`'s own cross-correlation confirm step already handles
    ///   without pre-filtering — see `detector_rejects_steady_tone`). Recovering
    ///   that margin (e.g. a cheaper/shorter continuous filter) is a reasonable
    ///   future improvement but out of Task 7's scope.
    /// - The one-shot (block) `Fir` filtering `CoppaTransceiver::demodulate_header`/
    ///   `receive_with_metrics` already do internally is applied only to each
    ///   candidate's own small slice, not the whole stream — the same total
    ///   filtering work as before, just paid once per real candidate instead of
    ///   once per incoming sample.
    ring: VecDeque<f32>,
    ring_base: u64,
    /// Total samples ever pushed (absolute coordinate origin).
    total_pushed: u64,
    /// Candidates confirmed by `sync` but not yet consumed by the state machine.
    candidates: VecDeque<SyncCandidate>,
    /// Searching never considers a candidate whose `frame_start` is before this —
    /// advanced past a candidate's data once it's been resolved (successfully or
    /// not), and by exactly `symbol_len` (not the whole buffered window) on a
    /// header decode failure, per the locked state machine. On a header-decode
    /// failure specifically, the `symbol_len` skip is clamped by
    /// [`Self::advance_resume_from_past`] so it never passes an already-queued
    /// later candidate — see that method's doc for why.
    resume_from: u64,
    pending: Option<Pending>,
    /// COP-68: candidates whose protected header decoded but whose payload
    /// failed FEC (`ReceiveError::LdpcNotConverged`/`CrcMismatch` -- the
    /// post-header kinds). From level 3 up under Watterson fading this
    /// "header decodes / payload fails" regime is real and SNR-independent,
    /// and it is the peer-evidence signal the daemon's session fade-hold
    /// keys on; pre-header failures (`SyncFailed`/`HeaderCorrupt`) never
    /// touch it. Monotone since construction; an engine rebuild (profile/
    /// level change) replaces this receiver and restarts the count, which is
    /// fine for the daemon's before/after-delta read.
    header_only_decodes: u32,
}

impl StreamingReceiver {
    pub fn new(profile: CoppaProfile, version: u8) -> Self {
        let symbol_len = profile.fft_size + profile.cp_samples;
        let data_carriers = profile.data_carriers;
        let header_syms = header_fec::PROTECTED_HEADER_CODED_BITS.div_ceil(data_carriers);
        let max_frame_samples = Self::max_frame_samples(&profile);
        // phy_mode 0 = HF/SSB; mirrors the same gate `CoppaTransceiver`/`CoppaModem`
        // use for their own RX/TX bandpass filters (`RX_BPF_TAPS`'s doc).
        let rx_group_delay = if profile.phy_mode == 0 {
            (RX_BPF_TAPS - 1) / 2
        } else {
            0
        };
        let sync = SyncDetector::new(profile.clone(), version);
        let transceiver = CoppaTransceiver::new(profile, version);

        // Cross-crate invariant: `data_carriers` (used here for frame-length
        // bookkeeping) must equal what `CoppaModem` computes internally as
        // `pilots.num_data()` (exposed via `CoppaTransceiver::data_carriers_per_symbol`).
        // Nothing enforces this at the type level — a future profile change that
        // breaks the coincidence would silently mis-size the ring/accumulation
        // buffers instead of failing loudly, so check it here.
        debug_assert_eq!(
            data_carriers,
            transceiver.data_carriers_per_symbol(),
            "StreamingReceiver's profile.data_carriers must match CoppaModem's \
             internally-computed data carriers per symbol"
        );

        Self {
            symbol_len,
            data_carriers,
            header_syms,
            max_frame_samples,
            rx_group_delay,
            sync,
            transceiver,
            ring: VecDeque::new(),
            ring_base: 0,
            total_pushed: 0,
            candidates: VecDeque::new(),
            resume_from: 0,
            pending: None,
            header_only_decodes: 0,
        }
    }

    /// COP-68: how many candidates since construction decoded a protected
    /// header but failed the payload FEC (the post-header `ReceiveError`
    /// kinds) -- the "never drop while a header decodes" evidence signal.
    /// See the field's doc for the rebuild-resets-the-count caveat.
    pub fn header_only_decodes(&self) -> u32 {
        self.header_only_decodes
    }

    /// Ring capacity: 2 × max frame length, computed from the profile + SPEED_LEVELS.
    ///
    /// Every frame is exactly `3 + header_syms + payload_syms(level, codewords)`
    /// OFDM symbols — `payload_len` only controls how many bytes are extracted
    /// from the resulting fixed-size (1944-coded-bit) LDPC blocks afterward, not
    /// how many symbols are transmitted (see `CoppaModem::demodulate_frame`'s
    /// doc). COP-69: `payload_syms` scales with the header's codeword count, so
    /// the worst case is the most robust speed level (BPSK) at
    /// [`MAX_CODEWORDS`] codewords — for `hf_standard` that is 361 symbols × 1260
    /// samples = 454,860 samples per frame, so the ring target is ~909,720
    /// samples ≈ 3.6 MB at 4 bytes/sample. That is a deliberate size decision,
    /// not an accident: it is what lets the daemon's real receive path hold a
    /// whole 8-codeword frame plus searching history (the pre-COP-69 bound was
    /// the single-codeword 65,520, under 1/3rd of one such frame).
    ///
    /// `payload_syms(level)` is OFDM symbols, not constellation (bit-group) symbols:
    /// 1944 coded bits pack into `1944.div_ceil(bits_per_symbol)` constellation
    /// symbols, which then further pack `data_carriers`-at-a-time into OFDM symbols
    /// — both divisions matter (an earlier version of this code dropped the second
    /// one and grossly overestimated every frame's length).
    pub fn max_frame_samples(profile: &CoppaProfile) -> usize {
        let symbol_len = profile.fft_size + profile.cp_samples;
        let header_syms = header_fec::PROTECTED_HEADER_CODED_BITS.div_ceil(profile.data_carriers);
        // `frame_symbol_count` is monotone in `codewords` for every level, so
        // evaluating at MAX_CODEWORDS yields each level's worst case; the max
        // over levels is then the profile's true bound.
        let max_syms = SPEED_LEVELS
            .iter()
            .map(|sl| {
                frame_symbol_count(
                    header_syms,
                    sl.bits_per_symbol as usize,
                    profile.data_carriers,
                    MAX_CODEWORDS as usize,
                )
            })
            .max()
            .unwrap_or(0);
        max_syms * symbol_len
    }

    /// Current number of raw samples retained in the ring (test/diagnostic hook).
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// Feed audio; returns zero or more completed frames.
    ///
    /// O(chunk) amortized *throughput*: the per-sample cost of buffering + sync
    /// detection is O(1), and the one O(frame length) demod/FEC pass per candidate
    /// is amortized over all the pushes that buffered it — so averaged over a
    /// session, the cost per sample pushed stays small.
    ///
    /// This is NOT a bound on *worst-case per-call latency*. Whichever call happens
    /// to supply the last sample a pending candidate needs pays the full
    /// `receive_with_metrics` demod/FEC cost synchronously, inline, before
    /// returning — measured at ~26-33ms for the slower/most robust speed levels on
    /// this profile. COP-69: a multi-codeword frame's completing call pays every
    /// codeword's LDPC pass, so the worst case grows toward ~8x that for an
    /// 8-codeword frame at the slowest level (amortized per-sample cost is
    /// unchanged — same total FEC work, just concentrated in one call). A caller
    /// on a latency-sensitive loop (e.g. an async event
    /// loop driving audio I/O) should expect an occasional call to stall for a
    /// full frame's decode time, not assume every call is cheap. See
    /// `coppa-daemon/src/event_loop.rs`'s `handle_audio_in` for how the daemon
    /// accepts this trade-off (ring-buffered input + an overflow counter) rather
    /// than routing the decode through a worker thread.
    pub fn push_samples(&mut self, samples: &[f32]) -> Vec<DecodedFrame> {
        let mut out = Vec::new();
        if samples.is_empty() {
            return out;
        }

        self.ring.extend(samples.iter().copied());
        self.total_pushed += samples.len() as u64;

        let new_candidates = self.sync.push(samples);
        self.candidates.extend(new_candidates);

        loop {
            if self.pending.is_none() {
                while matches!(self.candidates.front(), Some(c) if c.frame_start < self.resume_from)
                {
                    self.candidates.pop_front();
                }
                match self.candidates.pop_front() {
                    Some(c) => {
                        // `header_peek` reads up to `rx_group_delay + 3*symbol_len
                        // + header_syms*symbol_len` into this slice (the RX
                        // filter's group delay shifts its read window later by
                        // that much — see `header_peek`'s doc), so `need` must
                        // include that slack or the read falls off the end of a
                        // slice sized to the "nominal" (no-delay) header region.
                        let need = self.rx_group_delay + (3 + self.header_syms) * self.symbol_len;
                        self.pending = Some(Pending {
                            start: c.frame_start,
                            need,
                            cfo_hz: c.cfo_hz,
                            header: None,
                        });
                    }
                    None => break,
                }
            }

            let (start, need, cfo_hz, header_known) = {
                let p = self.pending.as_ref().expect("checked above");
                (p.start, p.need, p.cfo_hz, p.header.is_some())
            };

            if self.total_pushed.saturating_sub(start) < need as u64 {
                break; // need more samples before the next step can run
            }

            if start < self.ring_base {
                // Shouldn't happen given `evict_ring`'s policy, but never index before
                // the ring's retained history.
                self.advance_resume_from_past(start);
                self.pending = None;
                continue;
            }

            if !header_known {
                match self.header_peek(start, cfo_hz) {
                    Some(header) => {
                        match SPEED_LEVELS.iter().find(|s| s.level == header.speed_level) {
                            Some(sl) => {
                                // COP-69: the payload region scales with the
                                // header's codeword count (up to MAX_CODEWORDS),
                                // matching what the TX side and the batch demod
                                // both assume. Before this multiplier, a
                                // multi-codeword frame was "completed" after its
                                // first codeword's worth of samples and every
                                // such frame failed decode.
                                let full_need = frame_symbol_count(
                                    self.header_syms,
                                    sl.bits_per_symbol as usize,
                                    self.data_carriers,
                                    header.codewords.max(1) as usize,
                                ) * self.symbol_len;
                                let p = self.pending.as_mut().expect("checked above");
                                p.need = full_need;
                                p.header = Some(header);
                                // loop again: may already have enough for full_need
                            }
                            None => {
                                self.advance_resume_from_past(start);
                                self.pending = None;
                            }
                        }
                    }
                    None => {
                        // Header decode failure: discard this candidate and resume
                        // search right after just the first symbol, not the whole
                        // buffered header window.
                        self.advance_resume_from_past(start);
                        self.pending = None;
                    }
                }
            } else {
                let start_rel = (start - self.ring_base) as usize;
                let slice: Vec<f32> = self
                    .ring
                    .iter()
                    .skip(start_rel)
                    .take(need)
                    .copied()
                    .collect();
                // `receive_with_metrics` re-derives its own exact timing (and
                // applies its own one-shot RX-bandpass filter) via a fresh internal
                // `SyncDetector::detect_all` on whatever slice it's given — unlike
                // `header_peek` below, it needs no caller-supplied margin/offset
                // (this is exactly how every existing `CoppaTransceiver::receive`
                // unit test already calls it: directly on `transmit`'s raw output,
                // zero leading margin).
                match self.transceiver.receive_with_metrics(&slice) {
                    Ok((header, payload, snr_db, recommended_level, delay_spread_ms)) => {
                        out.push(DecodedFrame {
                            header,
                            payload,
                            snr_db,
                            recommended_level,
                            cfo_hz,
                            frame_start: start,
                            delay_spread_ms,
                        });
                    }
                    Err(e) => {
                        // Full frame demod/FEC failed; drop it and keep
                        // searching. COP-68: a POST-header failure (the
                        // header decoded, the payload did not) still proves
                        // someone is on frequency with a decodable preamble
                        // and header -- count it so the daemon can treat it
                        // as peer evidence. Pre-header failures
                        // (SyncFailed/HeaderCorrupt) prove nothing and are
                        // not counted.
                        if matches!(
                            e,
                            ReceiveError::LdpcNotConverged { .. } | ReceiveError::CrcMismatch
                        ) {
                            self.header_only_decodes += 1;
                        }
                    }
                }
                self.resume_from = start + need as u64;
                self.pending = None;
            }
        }

        self.evict_ring();
        out
    }

    /// Advance `resume_from` to `start + symbol_len`, but never past an
    /// already-queued later candidate.
    ///
    /// This is the recovery step after a header-decode failure at `start`: the
    /// state machine deliberately resumes search right after just the failed
    /// candidate's first symbol, not the whole buffered header window (see the
    /// call sites' comments). But `self.candidates` can already contain a
    /// genuine, not-yet-tried candidate whose `frame_start` is very close to
    /// `start` — a spurious candidate near the tail of one frame and the real
    /// start of the next can land within a few samples of each other,
    /// depending on inter-frame gap length and content. An unclamped skip can
    /// overshoot that real candidate's `frame_start`, and the very next loop
    /// iteration's staleness filter (`candidates.front() < resume_from`) would
    /// then discard it — silently losing a real frame (the confirmed root
    /// cause of a previously-reported spurious-second-decode bug affecting
    /// VHF speed levels 5 and 7 at specific inter-frame gap lengths). Clamping
    /// the skip to stop at, not past, the next queued candidate closes that
    /// gap without weakening the skip for the common case where no such
    /// candidate is waiting.
    fn advance_resume_from_past(&mut self, start: u64) {
        let proposed = start + self.symbol_len as u64;
        self.resume_from = match self.candidates.front() {
            Some(c) if c.frame_start > start => proposed.min(c.frame_start),
            _ => proposed,
        };
    }

    /// Demodulate just the header of the candidate starting at `start`.
    ///
    /// Unlike the full-frame `receive_with_metrics` path, [`CoppaTransceiver::
    /// demodulate_header`] takes an explicit `data_start` rather than finding its
    /// own timing via a fresh `SyncDetector::detect_all` — so unlike that path,
    /// this DOES need `data_start` to be exactly right. `start` itself is already
    /// correctly located in the RAW domain (that's what `SyncDetector` — also fed
    /// raw samples — determined, and its own tests confirm this works directly on
    /// unfiltered signals: `detector_prefers_strongest_path_at_realistic_hf_delay`
    /// and `detector_falls_back_to_first_path_beyond_half_cp` (in
    /// `coppa-codec/src/ofdm/sync_detector.rs`) both compute their expected
    /// position from `tx_bpf_group_delay` alone, no RX filter involved).
    /// But `demodulate_header` (like `receive`) runs its OWN one-shot RX-bandpass
    /// filter over whatever slice it's given, which — being a linear-phase FIR —
    /// delays the correctly-filtered content within ITS OWN output by exactly its
    /// group delay (300 samples for the 601-tap HF filter; 0 for VHF, no RX
    /// filter). So relative to a slice starting exactly at `start` (no extra
    /// leading margin needed or wanted — an earlier version of this code added an
    /// arbitrary large margin here, which overshot `data_start` by nearly 2
    /// symbols and broke every header decode; see the Task 7 report for the
    /// measured before/after), the header begins not at `3 * symbol_len` but at
    /// `rx_group_delay + 3 * symbol_len`.
    /// `cfo_hz` is the sync candidate's own two-stage Moose CFO estimate
    /// (`Pending::cfo_hz`, carried through from `SyncCandidate::cfo_hz`) —
    /// see below for why this slice needs its own CFO correction, separate
    /// from the full-frame path's.
    ///
    /// Review finding: this used to demodulate the raw (CFO-uncorrected)
    /// slice directly. `CoppaModem::demodulate_frame_impl` (the batch/
    /// full-search path `receive_with_metrics` below ultimately reaches)
    /// removes the candidate's estimated CFO from the whole buffer BEFORE
    /// calling `demodulate_header` -- residual CFO de-rotates every
    /// subcarrier and can break the header's hard-decision BPSK demod
    /// outright. Without the same correction here, `header_peek` silently
    /// discarded every candidate with a non-negligible CFO -- not just an
    /// intentionally-injected one (e.g. the golden vectors' `ssbcfo` channel,
    /// 15 Hz) but also a merely noise-induced nonzero sync CFO estimate at
    /// moderate SNR (e.g. the `awgn12` golden vectors, no injected CFO at
    /// all) -- even though the full-frame path (reached only if this peek
    /// succeeds) re-derives its own timing/CFO independently on the extended
    /// slice and would have handled it fine. Confirmed via
    /// `crates/coppa-cli/tests/rx_golden.rs`: those vectors decoded via the
    /// batch `CoppaTransceiver::receive` API (no `header_peek` in that path)
    /// but produced zero frames via `StreamingReceiver::push_samples` before
    /// this fix, despite `push_samples` being handed the exact same samples.
    /// `remove_cfo`'s correction is local to whatever slice it's given (a
    /// pure per-sample de-rotation from that slice's own index 0, not tied
    /// to absolute time) -- the same 0.5 Hz noise-floor gate
    /// `demodulate_frame_impl` uses avoids a needless pass when the estimate
    /// is negligible.
    ///
    /// # `SYNC_LEAD_MARGIN`/`HEADER_PEEK_ANCHOR_MARGIN` padding (confirmed VHF
    /// `TIMING_BACKOFF` bug fix, see
    /// `.superpowers/sdd/vhf-timing-backoff-fix-report.md`)
    ///
    /// For a profile with no TX bandpass filter (VHF, i.e. `self.rx_group_delay
    /// == 0` -- see that field's doc) ONLY: `demodulate_header` ->
    /// `demodulate_header_llrs` -> `probe_calibration` reads
    /// `CoppaModem::calibrated_bias`, which (post-fix, VHF only) is measured
    /// assuming at least `coppa_codec::ofdm::coppa_modem::SYNC_LEAD_MARGIN`
    /// samples of leading margin before the true preamble -- see
    /// `measure_bulk_bias`'s doc. `demodulate_frame_impl` (the full-frame path)
    /// guarantees that itself by padding its own incoming buffer (same VHF-only
    /// gate) before running a FRESH sync detection pass, which self-corrects the
    /// anchor regardless of how much margin was already there. This peek
    /// deliberately has no such fresh detection step (`start` already comes from
    /// the caller's own persistent, session-wide `SyncDetector`, and re-running
    /// full detection here would defeat the point of a cheap "peek" -- see this
    /// method's doc above), so for VHF it must instead reconstruct the SAME
    /// anchor `demodulate_frame_impl` would have found, using
    /// `HEADER_PEEK_ANCHOR_MARGIN` (`SYNC_LEAD_MARGIN - TIMING_BACKOFF`, see its
    /// doc for the full derivation and the confirmed-safe-in-practice caveat
    /// about `start` itself possibly already being partially backed off).
    /// Skipping this desyncs this peek's header decode from `calibrated_bias`'s
    /// real reference frame for VHF -- confirmed the hard way: an earlier
    /// version of this fix used `SYNC_LEAD_MARGIN` directly (no `-
    /// TIMING_BACKOFF` term) here, which passed every test in
    /// `coppa-codec`/`coppa-protocol` but broke every VHF golden vector in
    /// `crates/coppa-cli/tests/rx_golden.rs`. This is NOT the same historical
    /// mistake this doc's earlier paragraph warns about (adding an arbitrary
    /// margin here once broke header decode) -- that margin was never reflected
    /// into `data_start`; this one is, by construction, always exactly
    /// `HEADER_PEEK_ANCHOR_MARGIN`.
    ///
    /// HF profiles (`rx_group_delay > 0`) apply NEITHER padding here, matching
    /// `measure_bulk_bias`/`demodulate_frame_impl`'s own VHF-only gating on the
    /// codec side: their own RX bandpass filter's group delay already keeps
    /// everything comfortably clear of `TIMING_BACKOFF`'s saturation with zero
    /// padding, and applying it anyway was measured to cause a real regression
    /// (a marginal Watterson-Poor golden vector) -- see
    /// `CoppaModem::demodulate_frame_impl`'s "0." step doc for the specifics.
    fn header_peek(&self, start: u64, cfo_hz: f32) -> Option<CoppaHeader> {
        use coppa_codec::ofdm::coppa_modem::{HEADER_PEEK_ANCHOR_MARGIN, SYNC_LEAD_MARGIN};

        let need = self.pending.as_ref()?.need;
        let start_rel = (start - self.ring_base) as usize;
        let is_vhf = self.rx_group_delay == 0;
        let lead_pad = if is_vhf { SYNC_LEAD_MARGIN } else { 0 };
        let mut slice: Vec<f32> = Vec::with_capacity(lead_pad + need);
        slice.resize(lead_pad, 0.0);
        slice.extend(self.ring.iter().skip(start_rel).take(need).copied());

        let corrected;
        let slice: &[f32] = if cfo_hz.abs() > 0.5 {
            let sample_rate = self.transceiver.profile().sample_rate as f32;
            corrected = coppa_codec::ofdm::sync::remove_cfo(&slice, cfo_hz, sample_rate);
            &corrected
        } else {
            &slice
        };

        let anchor_margin = if is_vhf { HEADER_PEEK_ANCHOR_MARGIN } else { 0 };
        let data_start = anchor_margin + self.rx_group_delay + 3 * self.symbol_len;
        self.transceiver.demodulate_header(slice, data_start)
    }

    /// Evict ring samples no longer needed: from `pending.start` while accumulating,
    /// or, while searching, bounded by `max_frame_samples` of trailing history so an
    /// indefinitely noise-only stream doesn't grow the ring without bound (a
    /// candidate the `SyncDetector` confirms lags `total_pushed` by far less than a
    /// full frame length, so this bound never evicts data a real candidate needs).
    fn evict_ring(&mut self) {
        let keep_from = match &self.pending {
            Some(p) => p.start,
            None => self.resume_from.max(
                self.total_pushed
                    .saturating_sub(self.max_frame_samples as u64),
            ),
        };
        while self.ring_base < keep_from && !self.ring.is_empty() {
            self.ring.pop_front();
            self.ring_base += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modem::speed_levels::max_multi_payload_for_level;
    use coppa_codec::ofdm::frame::CoppaFrameType;

    fn header(speed_level: u8, payload_len: u16) -> CoppaHeader {
        CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level,
            seq_num: 0,
            payload_len,
            codewords: 1,
        }
    }

    fn make_frame(tx: &CoppaTransceiver, speed_level: u8, payload: &[u8]) -> Vec<f32> {
        let h = header(speed_level, payload.len() as u16);
        tx.transmit(&h, payload)
            .expect("payload within this test's speed level capacity")
    }

    /// COP-69 Phase 1 multi-codeword rig: a full-budget frame at
    /// (`speed_level`, `codewords`), same shape the batch-path multi-codeword
    /// tests in `transceiver.rs` build.
    fn make_multi_frame(
        tx: &CoppaTransceiver,
        speed_level: u8,
        payload: &[u8],
        codewords: u8,
    ) -> Vec<f32> {
        let mut h = header(speed_level, payload.len() as u16);
        h.codewords = codewords;
        tx.transmit(&h, payload)
            .expect("payload within this test's multi-codeword capacity")
    }

    /// A full-budget payload for (`speed_level`, `codewords`) with non-trivial
    /// content (not all-zero, not repeating single bytes).
    fn multi_payload(level: u8, codewords: u8) -> Vec<u8> {
        let max = max_multi_payload_for_level(level, codewords).expect("level on the ladder");
        (0..max).map(|i| (i % 251) as u8).collect()
    }

    /// Feed chunk sizes cycling through this list, regardless of how evenly they
    /// divide the total length.
    fn push_in_chunks(
        rx: &mut StreamingReceiver,
        samples: &[f32],
        chunk_sizes: &[usize],
    ) -> Vec<DecodedFrame> {
        let mut out = Vec::new();
        let mut i = 0;
        let mut chunk_idx = 0;
        while i < samples.len() {
            let want = chunk_sizes[chunk_idx % chunk_sizes.len()];
            chunk_idx += 1;
            let end = (i + want).min(samples.len());
            out.extend(rx.push_samples(&samples[i..end]));
            i = end;
        }
        out
    }

    #[test]
    fn streaming_decodes_frames_fed_in_odd_chunks() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload1 = b"Hello streaming frame one!".to_vec();
        let payload2 = b"And a second frame follows.".to_vec();
        let frame1 = make_frame(&tx, 2, &payload1);
        let frame2 = make_frame(&tx, 2, &payload2);

        let symbol_len_lead = profile.fft_size + profile.cp_samples;
        let mut stream = Vec::new();
        // Leading silence: the SyncDetector's Schmidl-Cox metric needs a clean
        // baseline in its bootstrap window (2*symbol_len) before the preamble
        // arrives, or the very first window (straddling silence+preamble) can
        // mis-time the plateau — see `sync_detector` module's own tests, which
        // all lead with >= 4*symbol_len of silence/gap before the first frame.
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len_lead));
        let frame1_start = stream.len() as u64;
        stream.extend_from_slice(&frame1);
        // 5000 samples of noise between the two frames.
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};
        let mut rng = StdRng::seed_from_u64(11);
        stream.extend((0..5000).map(|_| rng.random_range(-0.01f32..0.01f32)));
        let frame2_start = stream.len() as u64;
        stream.extend_from_slice(&frame2);
        // Trailing noise so the second frame's tail isn't the very end of input.
        stream.extend((0..2000).map(|_| rng.random_range(-0.01f32..0.01f32)));

        let mut rx = StreamingReceiver::new(profile, 1);
        let decoded = push_in_chunks(&mut rx, &stream, &[1, 7, 64, 480, 4096]);

        assert_eq!(
            decoded.len(),
            2,
            "expected exactly two decoded frames, got {}",
            decoded.len()
        );
        assert_eq!(decoded[0].payload[..payload1.len()], payload1[..]);
        assert_eq!(decoded[1].payload[..payload2.len()], payload2[..]);

        // frame_start should be close to the true preamble position (within one
        // symbol length: the candidate's own timing backoff/first-path refinement
        // introduces a small, already-tested offset — see `sync_detector` tests).
        let symbol_len =
            CoppaProfile::hf_standard().fft_size + CoppaProfile::hf_standard().cp_samples;
        assert!(
            (decoded[0].frame_start as i64 - frame1_start as i64).unsigned_abs()
                < symbol_len as u64,
            "frame1_start {} should be near {frame1_start}",
            decoded[0].frame_start
        );
        assert!(
            (decoded[1].frame_start as i64 - frame2_start as i64).unsigned_abs()
                < symbol_len as u64,
            "frame2_start {} should be near {frame2_start}",
            decoded[1].frame_start
        );
    }

    /// Regression test for the `header_peek` CFO fix (Phase 4 Task 4 review
    /// finding, see `header_peek`'s doc): a frame with a real injected CFO
    /// must still decode via the streaming path, not just the batch
    /// `CoppaTransceiver::receive` API. Before the fix, `header_peek` never
    /// applied the sync candidate's own CFO estimate before demodulating the
    /// header, so ANY candidate with a non-negligible CFO was silently
    /// discarded (header decode "failure") even though the full-frame path
    /// (reached only if the header peek succeeds) re-derives its own
    /// timing/CFO independently and would have decoded it fine.
    #[test]
    fn streaming_decodes_a_frame_with_injected_cfo() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload = b"CFO-affected streaming frame".to_vec();
        let frame = make_frame(&tx, 2, &payload);

        // Inject +15 Hz (matches testdata/golden/*_ssbcfo.wav's injected offset)
        // by removing -15 Hz, the same trick `sync.rs`'s own
        // `estimate_cfo_recovers_injected_offset` test uses.
        let cfo_frame =
            coppa_codec::ofdm::sync::remove_cfo(&frame, -15.0, profile.sample_rate as f32);

        let symbol_len_lead = profile.fft_size + profile.cp_samples;
        let mut stream = Vec::new();
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len_lead));
        stream.extend_from_slice(&cfo_frame);
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len_lead));

        let mut rx = StreamingReceiver::new(profile, 1);
        let decoded = rx.push_samples(&stream);

        assert_eq!(
            decoded.len(),
            1,
            "expected exactly one decoded frame despite the injected +15 Hz CFO"
        );
        assert_eq!(decoded[0].payload[..payload.len()], payload[..]);
    }

    #[test]
    fn streaming_memory_is_bounded() {
        let profile = CoppaProfile::hf_standard();
        let max_frame = StreamingReceiver::max_frame_samples(&profile);
        let mut rx = StreamingReceiver::new(profile.clone(), 1);

        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};
        let mut rng = StdRng::seed_from_u64(99);

        let sr = profile.sample_rate as usize;
        let total = 60 * sr; // 60 s of pure noise
        let chunk = 4096usize;
        let mut pushed = 0usize;
        while pushed < total {
            let n = chunk.min(total - pushed);
            let samples: Vec<f32> = (0..n)
                .map(|_| rng.random_range(-0.05f32..0.05f32))
                .collect();
            let frames = rx.push_samples(&samples);
            assert!(
                frames.is_empty(),
                "pure noise must not produce any decoded frames"
            );
            assert!(
                rx.len() <= 2 * max_frame,
                "ring len {} exceeded 2*max_frame_samples ({})",
                rx.len(),
                2 * max_frame
            );
            pushed += n;
        }
    }

    /// COP-68: the header-only decode counter moves only for post-header
    /// failures. A frame whose sample region after the protected header is
    /// destroyed (header is Golay-protected and intact, LDPC sees silence)
    /// increments it by exactly 1; a clean frame and pure noise (sync
    /// failures) leave it unchanged.
    #[test]
    fn header_only_decodes_counts_post_header_failures_only() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload = b"tail-corrupted frame payload".to_vec();
        let mut corrupted = make_frame(&tx, 2, &payload);

        // Destroy everything after the protected header region: the preamble
        // and header stay decodable, the payload LDPC sees silence. This is
        // the deterministic on-air shape of the level>=3 Watterson regime
        // the daemon's fade-hold keys on (research §4).
        let symbol_len = profile.fft_size + profile.cp_samples;
        let header_syms = header_fec::PROTECTED_HEADER_CODED_BITS.div_ceil(profile.data_carriers);
        let header_end = (3 + header_syms) * symbol_len;
        let cut = header_end.min(corrupted.len());
        for s in corrupted.iter_mut().skip(cut) {
            *s = 0.0;
        }

        let mut stream = Vec::new();
        // Leading silence — see the comment in `streaming_decodes_frames_fed_in_odd_chunks`.
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));
        stream.extend_from_slice(&corrupted);
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));

        let mut rx = StreamingReceiver::new(profile.clone(), 1);
        let before = rx.header_only_decodes();
        let decoded = rx.push_samples(&stream);
        assert!(decoded.is_empty(), "the corrupted frame must not decode");
        assert_eq!(
            rx.header_only_decodes(),
            before + 1,
            "a header-decodes/payload-fails candidate must count exactly once"
        );

        // A clean frame of the same payload decodes and does not bump it.
        let clean = make_frame(&tx, 2, &payload);
        let mut stream2 = Vec::new();
        stream2.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));
        stream2.extend_from_slice(&clean);
        stream2.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));
        let frames = rx.push_samples(&stream2);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload[..payload.len()], payload[..]);
        assert_eq!(
            rx.header_only_decodes(),
            before + 1,
            "a fully decoded frame must not count as header-only"
        );

        // Pure noise (sync failures, maybe header decode failures) never
        // touches the counter: neither kind is post-header evidence.
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};
        let mut rng = StdRng::seed_from_u64(11);
        let noise: Vec<f32> = (0..profile.sample_rate as usize)
            .map(|_| rng.random_range(-0.05f32..0.05f32))
            .collect();
        let frames = rx.push_samples(&noise);
        assert!(frames.is_empty());
        assert_eq!(
            rx.header_only_decodes(),
            before + 1,
            "pre-header failures must not count as evidence"
        );
    }

    #[test]
    fn header_failure_resumes_search_promptly() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload2 = b"Second frame after a corrupt header.".to_vec();
        let mut frame1 = make_frame(&tx, 2, b"first frame, header will be trashed");
        let frame2 = make_frame(&tx, 2, &payload2);

        // Badly corrupt frame1's header region (well beyond Golay(24,12)'s 3-error
        // correction budget per word) so `demodulate_header` reliably fails.
        let symbol_len = profile.fft_size + profile.cp_samples;
        let header_start = 3 * symbol_len;
        for i in 0..(6 * symbol_len).min(frame1.len().saturating_sub(header_start)) {
            frame1[header_start + i] = if i % 2 == 0 { 0.9 } else { -0.9 };
        }

        let mut stream = Vec::new();
        // Leading silence — see the comment in `streaming_decodes_frames_fed_in_odd_chunks`.
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));
        stream.extend_from_slice(&frame1);
        // 0.5 s gap before the second, clean frame.
        let gap = profile.sample_rate as usize / 2;
        stream.extend(std::iter::repeat_n(0.0f32, gap));
        stream.extend_from_slice(&frame2);
        stream.extend(std::iter::repeat_n(
            0.0f32,
            4 * (profile.fft_size + profile.cp_samples),
        ));

        let mut rx = StreamingReceiver::new(profile, 1);
        let decoded = push_in_chunks(&mut rx, &stream, &[512]);

        assert_eq!(
            decoded.len(),
            1,
            "only the second (clean) frame should decode, got {} frames",
            decoded.len()
        );
        assert_eq!(decoded[0].payload[..payload2.len()], payload2[..]);
    }

    // ---- COP-69 Phase 1: the streaming path learns `header.codewords` ----
    //
    // Before the fix, `full_need` computed the payload region for exactly one
    // codeword, so an N-codeword frame was "completed" after 1/N of its samples
    // and every multi-codeword frame failed decode — while the batch
    // `CoppaTransceiver::receive` path decoded the same audio perfectly
    // (research probe `crates/coppa-engine/examples/cop69_probe.rs`, part C:
    // 0 of 1 decoded). The ring bound had the same blind spot: it maximized
    // over single-codeword frames only.

    /// An 8-codeword level-8 HF frame (the ticket's 64-QAM case: 8 x 158 = 1264
    /// B) decodes through the streaming path in a single `push_samples` call.
    ///
    /// `CoppaProfile::hf_standard()` is pinned explicitly: `EngineConfig::
    /// default()` is `LevelDerived` and resolves level 8 to `vhf_wide`, which
    /// cost the research phase one false reading.
    #[test]
    fn streaming_decodes_eight_codeword_frame_hf_level8() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload = multi_payload(8, 8);
        let frame = make_multi_frame(&tx, 8, &payload, 8);

        let symbol_len = profile.fft_size + profile.cp_samples;
        let mut stream = Vec::new();
        // 8+ symbols of leading silence (see the bootstrap note in
        // `streaming_decodes_frames_fed_in_odd_chunks`).
        stream.extend(std::iter::repeat_n(0.0f32, 8 * symbol_len));
        stream.extend_from_slice(&frame);
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));

        let mut rx = StreamingReceiver::new(profile, 1);
        let decoded = rx.push_samples(&stream);

        assert_eq!(
            decoded.len(),
            1,
            "the 8-codeword frame must decode through the streaming path"
        );
        assert_eq!(decoded[0].header.codewords, 8);
        assert_eq!(decoded[0].payload[..payload.len()], payload[..]);
    }

    /// The same 8-codeword frame fed in 4800-sample pushes across many calls —
    /// the daemon's real feed pattern (`handle_audio_in`'s ring-sized reads).
    #[test]
    fn streaming_decodes_multi_codeword_fed_in_small_chunks() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload = multi_payload(8, 8);
        let frame = make_multi_frame(&tx, 8, &payload, 8);

        let symbol_len = profile.fft_size + profile.cp_samples;
        let mut stream = Vec::new();
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));
        stream.extend_from_slice(&frame);
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));

        let mut rx = StreamingReceiver::new(profile, 1);
        let decoded = push_in_chunks(&mut rx, &stream, &[4800]);

        assert_eq!(
            decoded.len(),
            1,
            "the 8-codeword frame must decode when fed in 4800-sample chunks"
        );
        assert_eq!(decoded[0].header.codewords, 8);
        assert_eq!(decoded[0].payload[..payload.len()], payload[..]);
    }

    /// An 8-codeword frame followed by a 1-codeword frame: both decode, and
    /// the second frame's candidate survives the first frame's much longer
    /// accumulation (the staleness filter and `resume_from` advance must not
    /// eat it, and the first frame's `need` must not swallow it).
    #[test]
    fn streaming_multi_codeword_back_to_back_frames() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload8 = multi_payload(8, 8);
        let payload1 = b"single follows the big frame".to_vec();
        let frame8 = make_multi_frame(&tx, 8, &payload8, 8);
        let frame1 = make_frame(&tx, 8, &payload1);

        let symbol_len = profile.fft_size + profile.cp_samples;
        let mut stream = Vec::new();
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));
        stream.extend_from_slice(&frame8);
        // ~4 symbols of silence between the two frames.
        stream.extend(std::iter::repeat_n(0.0f32, 5000));
        stream.extend_from_slice(&frame1);
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));

        let mut rx = StreamingReceiver::new(profile, 1);
        let decoded = push_in_chunks(&mut rx, &stream, &[4800]);

        assert_eq!(
            decoded.len(),
            2,
            "both the 8-codeword and the 1-codeword frame must decode, got {}",
            decoded.len()
        );
        assert_eq!(decoded[0].header.codewords, 8);
        assert_eq!(decoded[1].header.codewords, 1);
        assert_eq!(decoded[0].payload[..payload8.len()], payload8[..]);
        assert_eq!(decoded[1].payload[..payload1.len()], payload1[..]);
    }

    /// The ring bound must cover the longest frame the ladder can now produce:
    /// an 8-codeword BPSK frame (361 symbols on `hf_standard`). Before COP-69
    /// the bound was the single-codeword maximum (65,520 samples) — under a
    /// third of that frame's 454,860 samples.
    #[test]
    fn max_frame_samples_covers_eight_codeword_bpsk() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload = multi_payload(2, 8); // level 2 = BPSK, the longest frames
        let frame = make_multi_frame(&tx, 2, &payload, 8);

        let max = StreamingReceiver::max_frame_samples(&profile);
        assert!(
            max >= frame.len(),
            "max_frame_samples ({max}) must cover an 8-codeword BPSK frame ({})",
            frame.len()
        );
    }

    /// Trailing garbage after a long frame must neither evict nor corrupt the
    /// pending frame: the ring policy was written for <= 52-symbol frames, and
    /// a 66-symbol frame accumulates across many chunked pushes while noise
    /// (which may confirm spurious sync candidates) streams in behind it.
    #[test]
    fn streaming_multi_codeword_with_trailing_garbage() {
        let profile = CoppaProfile::hf_standard();
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let payload = multi_payload(8, 8);
        let frame = make_multi_frame(&tx, 8, &payload, 8);

        let symbol_len = profile.fft_size + profile.cp_samples;
        let max_frame = StreamingReceiver::max_frame_samples(&profile);
        let mut stream = Vec::new();
        stream.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));
        stream.extend_from_slice(&frame);
        // A frame-length-and-a-half of trailing noise after the frame.
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};
        let mut rng = StdRng::seed_from_u64(69);
        stream.extend(
            (0..frame.len() + frame.len() / 2).map(|_| rng.random_range(-0.05f32..0.05f32)),
        );

        let mut rx = StreamingReceiver::new(profile, 1);
        let decoded = push_in_chunks(&mut rx, &stream, &[4800]);

        assert_eq!(
            decoded.len(),
            1,
            "exactly the multi-codeword frame must decode, got {}",
            decoded.len()
        );
        assert_eq!(decoded[0].header.codewords, 8);
        assert_eq!(decoded[0].payload[..payload.len()], payload[..]);
        assert!(
            rx.len() <= 2 * max_frame,
            "ring len {} exceeded 2*max_frame_samples ({})",
            rx.len(),
            2 * max_frame
        );
    }
}
