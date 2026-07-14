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
//!    frame (payload symbol count from `SPEED_LEVELS`). Once buffered, the full
//!    `CoppaTransceiver::receive_with_metrics` runs on exactly that slice, is
//!    emitted as a [`DecodedFrame`], and the receiver returns to `Searching`.
use std::collections::VecDeque;

use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::ofdm::frame::CoppaHeader;
use coppa_codec::ofdm::header_fec;
use coppa_codec::ofdm::sync_detector::{SyncCandidate, SyncDetector};
use coppa_codec::ofdm::CoppaProfile;

use crate::modem::transceiver::CoppaTransceiver;

/// LDPC coded block length (Z=81, 24 base columns) — constant for all code rates;
/// mirrors `CoppaModem::demodulate_frame`'s own `CODED_BLOCK_LEN`.
const CODED_BLOCK_LEN: usize = 1944;

/// Tap count of the RX bandpass filter `CoppaTransceiver`/`CoppaModem` use for HF
/// profiles (`phy_mode == 0`) — mirrors the constant baked into both of those
/// (`coppa_dsp::fir::design_bandpass(601, ..)`). Needed here only to compute that
/// filter's group delay; see [`StreamingReceiver::header_peek`]'s doc.
const RX_BPF_TAPS: usize = 601;

/// One frame fully decoded by [`StreamingReceiver::push_samples`].
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub header: CoppaHeader,
    pub payload: Vec<u8>,
    /// `10*log10(1 / mean(per-carrier noise variance))`, from
    /// `CoppaTransceiver::receive_with_metrics`.
    pub snr_db: f32,
    /// Two-stage Moose CFO estimate (Hz) captured when this frame's candidate was
    /// first confirmed by the `SyncDetector` (see `sync_detector` module docs).
    pub cfo_hz: f32,
    /// Absolute sample index (in the coordinate system of every sample ever pushed
    /// to this receiver) of the frame's preamble start.
    pub frame_start: u64,
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
    /// Upper bound on any single frame's sample length for this profile (worst case
    /// across all 9 speed levels); used to keep the ring bounded while searching.
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
    /// header decode failure, per the locked state machine.
    resume_from: u64,
    pending: Option<Pending>,
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
        }
    }

    /// Ring capacity: 2 × max frame length, computed from the profile + SPEED_LEVELS.
    ///
    /// Every frame is exactly `3 + header_syms + payload_syms(level)` OFDM symbols —
    /// `payload_len` only controls how many bytes are extracted from the resulting
    /// fixed-size (1944-coded-bit) LDPC block afterward, not how many symbols are
    /// transmitted (see `CoppaModem::demodulate_frame`'s doc). So the worst case
    /// across all 9 speed levels is a genuine, profile-only upper bound.
    ///
    /// `payload_syms(level)` is OFDM symbols, not constellation (bit-group) symbols:
    /// 1944 coded bits pack into `1944.div_ceil(bits_per_symbol)` constellation
    /// symbols, which then further pack `data_carriers`-at-a-time into OFDM symbols
    /// — both divisions matter (an earlier version of this code dropped the second
    /// one and grossly overestimated every frame's length).
    pub fn max_frame_samples(profile: &CoppaProfile) -> usize {
        let symbol_len = profile.fft_size + profile.cp_samples;
        let header_syms = header_fec::PROTECTED_HEADER_CODED_BITS.div_ceil(profile.data_carriers);
        let max_payload_syms = SPEED_LEVELS
            .iter()
            .map(|sl| {
                CODED_BLOCK_LEN
                    .div_ceil(sl.bits_per_symbol as usize)
                    .div_ceil(profile.data_carriers)
            })
            .max()
            .unwrap_or(0);
        (3 + header_syms + max_payload_syms) * symbol_len
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
    /// this profile. A caller on a latency-sensitive loop (e.g. an async event
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
                self.resume_from = start + self.symbol_len as u64;
                self.pending = None;
                continue;
            }

            if !header_known {
                match self.header_peek(start, cfo_hz) {
                    Some(header) => {
                        match SPEED_LEVELS.iter().find(|s| s.level == header.speed_level) {
                            Some(sl) => {
                                let coded_symbols =
                                    CODED_BLOCK_LEN.div_ceil(sl.bits_per_symbol as usize);
                                let payload_syms = coded_symbols.div_ceil(self.data_carriers);
                                let full_need =
                                    (3 + self.header_syms + payload_syms) * self.symbol_len;
                                let p = self.pending.as_mut().expect("checked above");
                                p.need = full_need;
                                p.header = Some(header);
                                // loop again: may already have enough for full_need
                            }
                            None => {
                                self.resume_from = start + self.symbol_len as u64;
                                self.pending = None;
                            }
                        }
                    }
                    None => {
                        // Header decode failure: discard this candidate and resume
                        // search right after just the first symbol, not the whole
                        // buffered header window.
                        self.resume_from = start + self.symbol_len as u64;
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
                    Ok((header, payload, snr_db)) => {
                        out.push(DecodedFrame {
                            header,
                            payload,
                            snr_db,
                            cfo_hz,
                            frame_start: start,
                        });
                    }
                    Err(_) => {
                        // Full frame demod/FEC failed; drop it and keep searching.
                    }
                }
                self.resume_from = start + need as u64;
                self.pending = None;
            }
        }

        self.evict_ring();
        out
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
    fn header_peek(&self, start: u64, cfo_hz: f32) -> Option<CoppaHeader> {
        let need = self.pending.as_ref()?.need;
        let start_rel = (start - self.ring_base) as usize;
        let slice: Vec<f32> = self
            .ring
            .iter()
            .skip(start_rel)
            .take(need)
            .copied()
            .collect();

        let corrected;
        let slice: &[f32] = if cfo_hz.abs() > 0.5 {
            let sample_rate = self.transceiver.profile().sample_rate as f32;
            corrected = coppa_codec::ofdm::sync::remove_cfo(&slice, cfo_hz, sample_rate);
            &corrected
        } else {
            &slice
        };

        let data_start = self.rx_group_delay + 3 * self.symbol_len;
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
}
