//! AFSK 1200 baud modem (Bell 202) for AX.25 / APRS.
//!
//! Implements HDLC framing (flags + bit stuffing), NRZI encoding,
//! and continuous-phase FSK audio generation at 48 kHz.
//!
//! The streaming demodulator band-pass filters the input, mixes each tone to
//! baseband through a root-raised-cosine matched filter, and tracks each
//! tone's level. Six parallel slicers (one on the level-normalised tones,
//! five on fixed mark-to-space gains) each recover their own symbol clock
//! and run their own HDLC and FX.25 receiver. A merge step delivers each
//! reception once.
use std::{collections::VecDeque, f32::consts::PI};

pub use crate::hdlc::append_fcs;
// The AFSK receiver keeps its own HDLC parser: FX.25 needs bit positions
// and reception spans that the shared HdlcDecoder does not expose.
use crate::hdlc::{bit_stuff, bits_to_byte_lsb, byte_to_bits_lsb, nrzi_encode};
#[cfg(test)]
use crate::hdlc::{bit_unstuff, nrzi_decode};
use crc::{Crc, CRC_16_IBM_SDLC};

const BAUD_RATE: u32 = 1200;
const MARK_FREQ: f32 = 1200.0; // mark = 1 (no transition in NRZI)
const SPACE_FREQ: f32 = 2200.0; // space = 0 (transition in NRZI)
const SAMPLE_RATE: u32 = 48000;
const SAMPLES_PER_SYMBOL: u32 = SAMPLE_RATE / BAUD_RATE; // 40
const HDLC_FLAG: u8 = 0x7E;
const PREAMBLE_FLAGS: usize = 36; // ~300 ms at 1200 baud
const POSTAMBLE_FLAGS: usize = 2;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Modulate raw AX.25 frame bytes (CRC already appended, e.g. by [`append_fcs`])
/// into 48 kHz audio.
///
/// Frame structure produced:
///   [preamble flags] [bit-stuffed frame body] [postamble flags]
///
/// The entire bit stream is NRZI-encoded before FSK generation.  Phase is
/// maintained continuously across symbol boundaries to avoid spectral splatter.
pub fn modulate(frame_bytes: &[u8]) -> Vec<f32> {
    // --- 1. Build raw bit stream -------------------------------------------

    // Flag bits (not bit-stuffed): 0x7E LSB-first = 0,1,1,1,1,1,1,0
    let flag_bits: Vec<u8> = byte_to_bits_lsb(HDLC_FLAG).to_vec();

    let mut bits: Vec<u8> = Vec::new();

    // Preamble
    for _ in 0..PREAMBLE_FLAGS {
        bits.extend_from_slice(&flag_bits);
    }

    // Frame body with bit stuffing
    let mut frame_bits: Vec<u8> = Vec::with_capacity(frame_bytes.len() * 8);
    for &byte in frame_bytes {
        frame_bits.extend_from_slice(&byte_to_bits_lsb(byte));
    }
    let stuffed = bit_stuff(&frame_bits);
    bits.extend_from_slice(&stuffed);

    // Postamble
    for _ in 0..POSTAMBLE_FLAGS {
        bits.extend_from_slice(&flag_bits);
    }

    bits_to_audio(&bits)
}

fn bits_to_audio(bits: &[u8]) -> Vec<f32> {
    // --- 2. NRZI encode -------------------------------------------------------
    let symbols = nrzi_encode(bits);

    // --- 3. Generate continuous-phase FSK audio -------------------------------
    let total_samples = symbols.len() * SAMPLES_PER_SYMBOL as usize;
    let mut samples = Vec::with_capacity(total_samples);
    let mut phase: f32 = 0.0;

    for &mark in &symbols {
        let freq = if mark { MARK_FREQ } else { SPACE_FREQ };
        let phase_increment = 2.0 * PI * freq / SAMPLE_RATE as f32;
        for _ in 0..SAMPLES_PER_SYMBOL {
            samples.push(phase.sin());
            phase += phase_increment;
            // Keep phase in [0, 2π) to prevent float drift over long transmissions.
            if phase >= 2.0 * PI {
                phase -= 2.0 * PI;
            }
        }
    }

    samples
}

/// Failure to construct an FX.25 frame.
#[derive(Debug, thiserror::Error)]
pub enum Fx25Error {
    /// Stuffed frame and inner flags exceed 223 bytes.
    #[error("AX.25 frame does not fit FX.25 with 32 parity bytes")]
    FrameTooLong,
    /// Input must include a valid AX.25 FCS.
    #[error("AX.25 frame has an invalid FCS")]
    InvalidFcs,
}

/// Encode a frame including its FCS as FX.25, mono 48 kHz AFSK 1200.
/// Uses the shortest fitting profile with 32 parity bytes.
pub fn modulate_fx25(frame_bytes: &[u8]) -> Result<Vec<f32>, Fx25Error> {
    if !valid_fcs(frame_bytes) {
        return Err(Fx25Error::InvalidFcs);
    }
    let flag = byte_to_bits_lsb(HDLC_FLAG);
    let mut inner = flag.to_vec();
    inner.extend(bit_stuff(
        &frame_bytes
            .iter()
            .flat_map(|&b| byte_to_bits_lsb(b))
            .collect::<Vec<_>>(),
    ));
    inner.extend(flag);
    let profile = [7, 6, 5, 4]
        .into_iter()
        .map(|i| crate::fx25::PROFILES[i])
        .find(|p| inner.len() <= p.data * 8)
        .ok_or(Fx25Error::FrameTooLong)?;
    let mut i = 0;
    while inner.len() < profile.data * 8 {
        inner.push(flag[i % 8]);
        i += 1;
    }
    let data: Vec<_> = inner.chunks_exact(8).map(bits_to_byte_lsb).collect();
    let block = crate::fx25::encode(&data, profile);
    let mut outer = flag.repeat(PREAMBLE_FLAGS);
    outer.extend(
        profile
            .tag
            .to_le_bytes()
            .iter()
            .flat_map(|&b| byte_to_bits_lsb(b)),
    );
    outer.extend(block.iter().flat_map(|&b| byte_to_bits_lsb(b)));
    outer.extend(flag.repeat(POSTAMBLE_FLAGS));
    Ok(bits_to_audio(&outer))
}

fn valid_fcs(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && append_fcs(&bytes[..bytes.len() - 2]) == bytes
}

#[derive(Debug)]
struct ReceivedFrame {
    bytes: Vec<u8>,
    start: u64,
    end: u64,
}

#[derive(Debug)]
struct FxReception {
    profile: crate::fx25::Profile,
    start: u64,
}

// Every defined profile has at most 255 transmitted bytes. Competing tags
// retain only start positions; their bits share this one bounded window.
const FX_WINDOW_BITS: usize = 255 * 8;

// Bit-to-sample history per lane. A delivered frame ends at most one FX.25
// window back, so the window plus a small margin always covers it.
const BIT_SAMPLE_HISTORY: usize = FX_WINDOW_BITS + 64;

// ---------------------------------------------------------------------------
// Receive front end
// ---------------------------------------------------------------------------
//
// The constants are the published values of Direwolf's profile A (WB2OSZ,
// "A Better APRS Packet Demodulator, Part 1"), used as numbers only.
// COP-113 measured them on WA8LMF TNC Test CD track 2.

// Pre-filter edges: the tones minus and plus 0.155 baud (186 Hz).
const BANDPASS_LOW_HZ: f64 = 1014.0;
const BANDPASS_HIGH_HZ: f64 = 2386.0;
// 417 taps at 48 kHz, rectangular window. Scored 1000 on track 2 against 991
// for a Blackman window of the same length.
const BANDPASS_TAPS: usize = 417;
// Unity gain at the centre of the band, half-way between the tones.
const BANDPASS_CENTRE_HZ: f64 = 1700.0;
// Root-raised-cosine matched filter on each tone's I and Q: roll-off 0.2,
// 2.8 symbols wide (113 taps at 40 samples per symbol).
const RRC_ROLL_OFF: f64 = 0.2;
const RRC_TAPS: usize = 113;
// Delay from the input to the front end's output, in samples: half of each
// linear-phase filter.
#[cfg(test)]
const FRONT_END_DELAY: u64 = ((BANDPASS_TAPS - 1) / 2 + (RRC_TAPS - 1) / 2) as u64;
// 240 samples hold a whole number of cycles of both tones at 48 kHz (6 of
// 1200 Hz, 11 of 2200 Hz), so a 240-entry table is an exact oscillator.
const MIXER_PERIOD: usize = 240;
// Per-tone level tracker: fast attack, slow decay (profile A).
const LEVEL_ATTACK: f32 = 0.70;
const LEVEL_DECAY: f32 = 0.000_09;
// Tracked levels below this are flushed to zero. Through long digital
// silence the slow decay would otherwise stall in subnormal floats, which
// are slow on x86. Real audio never comes near this level.
const LEVEL_FLOOR: f32 = 1e-30;
// Symbol clock: on every change of the decision, the clock's distance from
// the half-symbol point shrinks by this factor (profile A while receiving).
const DPLL_INERTIA: f32 = 0.74;
const SYMBOL_PERIOD: f32 = SAMPLES_PER_SYMBOL as f32;
const TRANSITION_TARGET: f32 = SYMBOL_PERIOD / 2.0;
// Gain slicers decide on `mark - g * space`. These are steps 2 to 6 of the
// geometric ladder 0.5 * 8^(k/8), k = 0..=8. With the level-normalised lane
// they decoded 1000 frames on track 2; nine gains decoded no more.
const SLICER_GAINS: [f32; 5] = [0.841, 1.091, 1.414, 1.834, 2.378];
const LANES: usize = SLICER_GAINS.len() + 1;
// Two lanes report the same reception when their frames carry the same
// bytes and end within 10 symbols. Separate receptions of one frame end at
// least 160 bits (6400 samples) apart: the smallest valid frame plus a flag.
const MERGE_TOLERANCE_SAMPLES: u64 = 10 * SAMPLES_PER_SYMBOL as u64;
// Delivered receptions stay in the merge history for 3 s, longer than the
// largest FX.25 block (2040 bits, 1.7 s): an FX.25 lane reports its corrected
// copy only after the whole block, well after a plain lane reported the
// same inner frame at its closing flag.
const MERGE_HISTORY_SAMPLES: u64 = 3 * SAMPLE_RATE as u64;

/// Linear-phase FIR filter over a double-length ring buffer, so every output
/// is one contiguous dot product.
struct Fir {
    taps: Vec<f32>,
    ring: Vec<f32>,
    pos: usize,
}

impl std::fmt::Debug for Fir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fir")
            .field("taps", &self.taps.len())
            .finish_non_exhaustive()
    }
}

impl Fir {
    fn new(taps: Vec<f32>) -> Self {
        let ring = vec![0.0; 2 * taps.len()];
        Self { taps, ring, pos: 0 }
    }

    fn push(&mut self, x: f32) -> f32 {
        let n = self.taps.len();
        self.pos = if self.pos == 0 { n - 1 } else { self.pos - 1 };
        self.ring[self.pos] = x;
        self.ring[self.pos + n] = x;
        // window[k] is the sample k pushes ago.
        let window = &self.ring[self.pos..self.pos + n];
        let mut acc = [0.0_f32; 8];
        let taps = self.taps.chunks_exact(8);
        let rest = taps.remainder();
        let samples = window.chunks_exact(8);
        let tail = samples.remainder();
        for (t, s) in taps.zip(samples) {
            for i in 0..8 {
                acc[i] += t[i] * s[i];
            }
        }
        let mut y = acc.iter().sum::<f32>();
        for (t, s) in rest.iter().zip(tail) {
            y += t * s;
        }
        y
    }
}

/// Ideal (truncated sinc) band-pass, rectangular window, scaled to unity
/// gain at [`BANDPASS_CENTRE_HZ`].
fn bandpass_taps() -> Vec<f32> {
    let fs = SAMPLE_RATE as f64;
    let mid = (BANDPASS_TAPS / 2) as f64;
    let low = BANDPASS_LOW_HZ / fs;
    let high = BANDPASS_HIGH_HZ / fs;
    let lowpass = |fc: f64, t: f64| {
        if t == 0.0 {
            2.0 * fc
        } else {
            (std::f64::consts::TAU * fc * t).sin() / (std::f64::consts::PI * t)
        }
    };
    let taps: Vec<f64> = (0..BANDPASS_TAPS)
        .map(|n| {
            let t = n as f64 - mid;
            lowpass(high, t) - lowpass(low, t)
        })
        .collect();
    let gain = response(&taps, BANDPASS_CENTRE_HZ);
    taps.iter().map(|&h| (h / gain) as f32).collect()
}

/// Root-raised-cosine impulse response, taps normalised to sum to 1.
fn matched_filter_taps() -> Vec<f32> {
    let beta = RRC_ROLL_OFF;
    let period = SAMPLES_PER_SYMBOL as f64;
    let mid = (RRC_TAPS / 2) as f64;
    let pi = std::f64::consts::PI;
    let taps: Vec<f64> = (0..RRC_TAPS)
        .map(|n| {
            let t = (n as f64 - mid) / period;
            if t == 0.0 {
                1.0 - beta + 4.0 * beta / pi
            } else if ((4.0 * beta * t).abs() - 1.0).abs() < 1e-9 {
                beta / 2f64.sqrt()
                    * ((1.0 + 2.0 / pi) * (pi / (4.0 * beta)).sin()
                        + (1.0 - 2.0 / pi) * (pi / (4.0 * beta)).cos())
            } else {
                ((pi * t * (1.0 - beta)).sin() + 4.0 * beta * t * (pi * t * (1.0 + beta)).cos())
                    / (pi * t * (1.0 - (4.0 * beta * t).powi(2)))
            }
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter().map(|&h| (h / sum) as f32).collect()
}

/// Magnitude of a filter's frequency response at `hz`.
fn response(taps: &[f64], hz: f64) -> f64 {
    let w = std::f64::consts::TAU * hz / SAMPLE_RATE as f64;
    let (re, im) = taps
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (n, &h)| {
            (re + h * (w * n as f64).cos(), im - h * (w * n as f64).sin())
        });
    re.hypot(im)
}

/// Mixer table entry `k`: cos and sin of both tones at sample `k`.
#[derive(Debug, Clone, Copy)]
struct MixerEntry {
    mark_cos: f32,
    mark_sin: f32,
    space_cos: f32,
    space_sin: f32,
}

fn mixer_table() -> Vec<MixerEntry> {
    let fs = SAMPLE_RATE as f64;
    (0..MIXER_PERIOD)
        .map(|k| {
            let mark = std::f64::consts::TAU * MARK_FREQ as f64 * k as f64 / fs;
            let space = std::f64::consts::TAU * SPACE_FREQ as f64 * k as f64 / fs;
            MixerEntry {
                mark_cos: mark.cos() as f32,
                mark_sin: mark.sin() as f32,
                space_cos: space.cos() as f32,
                space_sin: space.sin() as f32,
            }
        })
        .collect()
}

/// Fast-attack, slow-decay peak and valley tracker for one tone's level.
#[derive(Debug, Default)]
struct ToneLevel {
    peak: f32,
    valley: f32,
}

impl ToneLevel {
    /// Updates the tracker with `x` and returns `x` normalised to the
    /// tracked range, centred on zero.
    fn norm(&mut self, x: f32) -> f32 {
        if !x.is_finite() {
            // Never let a bad level poison the trackers for good.
            return 0.0;
        }
        let step = |level: f32, rate: f32| {
            let next = level + (x - level) * rate;
            if next.abs() < LEVEL_FLOOR {
                0.0
            } else {
                next
            }
        };
        self.peak = step(
            self.peak,
            if x >= self.peak {
                LEVEL_ATTACK
            } else {
                LEVEL_DECAY
            },
        );
        self.valley = step(
            self.valley,
            if x <= self.valley {
                LEVEL_ATTACK
            } else {
                LEVEL_DECAY
            },
        );
        if self.peak <= self.valley {
            0.0
        } else {
            (x - (self.peak + self.valley) / 2.0) / (self.peak - self.valley)
        }
    }
}

/// One slicer with its own symbol clock and bit receiver.
#[derive(Debug)]
struct Lane {
    /// `None` for the level-normalised lane, else the space-tone gain.
    gain: Option<f32>,
    clock: f32,
    prev_sign: bool,
    prev_symbol: bool,
    rx: BitReceiver,
}

impl Lane {
    fn new(gain: Option<f32>) -> Self {
        Self {
            gain,
            clock: 0.0,
            prev_sign: true,
            prev_symbol: true, // initial NRZI state = mark
            rx: BitReceiver::new(),
        }
    }

    /// Advances the lane by one sample. `sample` is the input sample count
    /// so far, recorded against any bit decided now.
    fn step(&mut self, soft: f32, sample: u64) {
        let sign = soft > 0.0;
        self.clock += 1.0;
        if self.clock >= SYMBOL_PERIOD {
            self.clock -= SYMBOL_PERIOD;
            // NRZI decode: same symbol as previous = 1, a change = 0.
            let bit = u8::from(sign == self.prev_symbol);
            self.prev_symbol = sign;
            self.rx.sample = sample;
            self.rx.receive_bit(bit);
        }
        if sign != self.prev_sign {
            // Pull the clock so transitions land half-way between decisions.
            self.clock = TRANSITION_TARGET + (self.clock - TRANSITION_TARGET) * DPLL_INERTIA;
        }
        self.prev_sign = sign;
    }
}

/// Delivers each reception once across lanes. Keyed on where a frame ended
/// in the input, not on when a lane reported it, because an FX.25 lane
/// reports late.
#[derive(Debug, Default)]
struct Merge {
    /// (bytes, end sample, delivered at sample).
    recent: VecDeque<(Vec<u8>, u64, u64)>,
}

impl Merge {
    /// Returns true when `bytes` ending at `end` is a new reception.
    fn offer(&mut self, bytes: &[u8], end: u64, now: u64) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|&(_, _, at)| now.saturating_sub(at) > MERGE_HISTORY_SAMPLES)
        {
            self.recent.pop_front();
        }
        if self
            .recent
            .iter()
            .any(|(b, e, _)| e.abs_diff(end) <= MERGE_TOLERANCE_SAMPLES && b == bytes)
        {
            return false;
        }
        self.recent.push_back((bytes.to_vec(), end, now));
        true
    }
}

// ---------------------------------------------------------------------------
// Demodulator
// ---------------------------------------------------------------------------

/// Streaming AFSK 1200 demodulator with HDLC frame extraction.
///
/// Feed audio samples via [`Demodulator::process`], then call
/// [`Demodulator::take_frames`] to retrieve any fully decoded frames.
#[derive(Debug)]
pub struct Demodulator {
    bandpass: Fir,
    mark_i: Fir,
    mark_q: Fir,
    space_i: Fir,
    space_q: Fir,
    mixer: Vec<MixerEntry>,
    mixer_index: usize,
    mark_level: ToneLevel,
    space_level: ToneLevel,
    lanes: [Lane; LANES],
    merge: Merge,
    // Input samples consumed so far.
    sample: u64,
    // Delivered frames with the input sample where each ended.
    frames: Vec<(Vec<u8>, u64)>,
}

impl Default for Demodulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Demodulator {
    /// Creates a demodulator in the idle state: no frame in progress, NRZI
    /// reference set to mark, and no decoded frames queued.
    pub fn new() -> Self {
        let rrc = matched_filter_taps();
        Self {
            bandpass: Fir::new(bandpass_taps()),
            mark_i: Fir::new(rrc.clone()),
            mark_q: Fir::new(rrc.clone()),
            space_i: Fir::new(rrc.clone()),
            space_q: Fir::new(rrc),
            mixer: mixer_table(),
            mixer_index: 0,
            mark_level: ToneLevel::default(),
            space_level: ToneLevel::default(),
            // Lane 0 is the level-normalised lane; lanes 1.. carry the gains.
            lanes: std::array::from_fn(|i| Lane::new(i.checked_sub(1).map(|g| SLICER_GAINS[g]))),
            merge: Merge::default(),
            sample: 0,
            frames: Vec::new(),
        }
    }

    /// Feed audio samples into the demodulator.
    pub fn process(&mut self, samples: &[f32]) {
        for &x in samples {
            // A NaN or infinite input sample would stay in the filter rings
            // for hundreds of samples; treat it as silence instead.
            let x = if x.is_finite() { x } else { 0.0 };
            self.sample += 1;
            let bp = self.bandpass.push(x);
            let m = self.mixer[self.mixer_index];
            self.mixer_index = (self.mixer_index + 1) % MIXER_PERIOD;
            let mark = self
                .mark_i
                .push(bp * m.mark_cos)
                .hypot(self.mark_q.push(bp * m.mark_sin));
            let space = self
                .space_i
                .push(bp * m.space_cos)
                .hypot(self.space_q.push(bp * m.space_sin));
            let norm_mark = self.mark_level.norm(mark);
            let norm_space = self.space_level.norm(space);
            for lane in &mut self.lanes {
                let soft = match lane.gain {
                    None => norm_mark - norm_space,
                    Some(g) => mark - g * space,
                };
                lane.step(soft, self.sample);
                if !lane.rx.frames.is_empty() {
                    for (bytes, end) in lane.rx.take_received() {
                        if self.merge.offer(&bytes, end, self.sample) {
                            self.frames.push((bytes, end));
                        }
                    }
                }
            }
        }
    }

    /// Delivered frames with the input sample where each ended.
    #[cfg(test)]
    fn take_received(&mut self) -> Vec<(Vec<u8>, u64)> {
        std::mem::take(&mut self.frames)
    }

    /// Extract all completed frames, draining the internal buffer.
    pub fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
            .into_iter()
            .map(|(bytes, _)| bytes)
            .collect()
    }

    /// Reset the demodulator to its initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ---------------------------------------------------------------------------
// Bit receiver: HDLC and FX.25 for one lane
// ---------------------------------------------------------------------------

/// HDLC and FX.25 state for one stream of decided bits.
#[derive(Debug)]
struct BitReceiver {
    // HDLC state machine
    in_frame: bool,      // currently receiving frame data
    bit_buffer: Vec<u8>, // accumulated data bits for current frame
    ones_count: u32,     // consecutive 1-bits for flag/stuffing detection

    fx_tag: u64,
    fx_tag_bits: usize,
    fx: Vec<FxReception>,
    fx_bits: VecDeque<u8>,
    pending_plain: Vec<ReceivedFrame>,
    delivered_spans: VecDeque<(u64, u64)>,

    // Completed frames
    frames: Vec<ReceivedFrame>,
    bit_position: u64,
    hdlc_start: u64,

    // Input sample index of each recent bit; the back is `bit_position`.
    bit_samples: VecDeque<u64>,
    // Input sample index of the bit being received.
    sample: u64,
}

impl BitReceiver {
    fn new() -> Self {
        Self {
            in_frame: false,
            bit_buffer: Vec::new(),
            ones_count: 0,
            frames: Vec::new(),
            bit_position: 0,
            hdlc_start: 0,
            fx_tag: 0,
            fx_tag_bits: 0,
            fx: Vec::new(),
            fx_bits: VecDeque::new(),
            pending_plain: Vec::new(),
            delivered_spans: VecDeque::new(),
            bit_samples: VecDeque::new(),
            sample: 0,
        }
    }

    /// Drains delivered frames as (bytes, input sample where the frame ended).
    fn take_received(&mut self) -> Vec<(Vec<u8>, u64)> {
        let frames = std::mem::take(&mut self.frames);
        frames
            .into_iter()
            .map(|frame| {
                let back = self.bit_position.saturating_sub(frame.end) as usize;
                // Plain frames end at the current bit and FX.25 frames at
                // most one FX.25 window back, so the history covers them.
                debug_assert!(back < self.bit_samples.len(), "frame end {back} bits back");
                let end = self
                    .bit_samples
                    .len()
                    .checked_sub(back + 1)
                    .and_then(|i| self.bit_samples.get(i))
                    .or(self.bit_samples.front())
                    .copied()
                    .unwrap_or(self.sample);
                (frame.bytes, end)
            })
            .collect()
    }

    fn emit_frame(&mut self, frame: ReceivedFrame) {
        let span = (frame.start, frame.end);
        if !self.delivered_spans.contains(&span) {
            self.delivered_spans.push_back(span);
            self.frames.push(frame);
        }
    }

    fn receive_bit(&mut self, bit: u8) {
        self.bit_position += 1;
        self.bit_samples.push_back(self.sample);
        if self.bit_samples.len() > BIT_SAMPLE_HISTORY {
            self.bit_samples.pop_front();
        }
        // Keep only reception associations that a maximum-size FX.25 block
        // could still reproduce. Identical later transmissions have new spans.
        self.delivered_spans
            .retain(|&(_, end)| self.bit_position - end <= FX_WINDOW_BITS as u64);
        self.hdlc_process_bit(bit);
        self.fx_bits.push_back(bit);
        if self.fx_bits.len() > FX_WINDOW_BITS {
            self.fx_bits.pop_front();
        }
        let candidates = std::mem::take(&mut self.fx);
        for rx in candidates {
            let length = (rx.profile.data + rx.profile.parity) * 8;
            if self.bit_position - rx.start < length as u64 {
                self.fx.push(rx);
                continue;
            }
            let bits: Vec<_> = self
                .fx_bits
                .iter()
                .skip(self.fx_bits.len() - length)
                .copied()
                .collect();
            let block: Vec<_> = bits.chunks_exact(8).map(bits_to_byte_lsb).collect();
            let corrected = crate::fx25::decode(&block, rx.profile).and_then(|data| {
                // Reuse ordinary HDLC/FCS validation without tag detection.
                let mut parser = Self::new();
                for b in data {
                    for bit in byte_to_bits_lsb(b) {
                        parser.bit_position += 1;
                        parser.hdlc_process_bit(bit);
                    }
                }
                parser.frames.into_iter().next().map(|mut frame| {
                    frame.start += rx.start;
                    frame.end += rx.start;
                    frame
                })
            });
            if let Some(frame) = corrected {
                self.pending_plain.retain(|plain| {
                    !(plain.start == frame.start
                        && plain.end == frame.end
                        && plain.bytes == frame.bytes)
                });
                self.emit_frame(frame);
            }
        }
        let pending = std::mem::take(&mut self.pending_plain);
        for frame in pending {
            if self.fx.iter().any(|rx| {
                rx.start < frame.start
                    && frame.end <= rx.start + ((rx.profile.data + rx.profile.parity) * 8) as u64
            }) {
                self.pending_plain.push(frame);
            } else {
                self.emit_frame(frame);
            }
        }
        self.fx_tag = (self.fx_tag >> 1) | ((bit as u64) << 63);
        self.fx_tag_bits = (self.fx_tag_bits + 1).min(64);
        if self.fx_tag_bits == 64 {
            if let Some(profile) = crate::fx25::find_tag(self.fx_tag) {
                // Keep competing starts: even a correctable damaged block can
                // contain a second tag. All candidates expire within one window,
                // with at most one start per incoming bit.
                self.fx.push(FxReception {
                    start: self.bit_position,
                    profile,
                });
            }
        }
    }

    /// HDLC bit-level state machine using ones-counting approach.
    ///
    /// Strategy: track consecutive 1-bits. Only push data bits when we know
    /// they are not part of a flag or stuffing. The key rules:
    ///   - 1-bits with ones_count < 5: push as data, increment count
    ///   - 0-bit after ones_count == 5: stuffed zero, discard (data 1s already pushed)
    ///   - 0-bit after ones_count == 6: flag! Remove last 5 pushed 1s from buffer
    ///   - ones_count reaches 7+: abort
    fn hdlc_process_bit(&mut self, bit: u8) {
        if bit == 1 {
            self.ones_count += 1;
            if self.ones_count >= 7 {
                // Abort: 7+ consecutive 1s
                self.in_frame = false;
                self.bit_buffer.clear();
                return;
            }
            // Push the 1-bit as data. For the 6th consecutive 1, we also push
            // it tentatively; if it turns out to be part of a flag (next bit 0),
            // we'll remove 6 ones from the buffer.
            if self.in_frame {
                self.bit_buffer.push(1);
            }
        } else {
            // bit == 0
            if self.ones_count >= 7 {
                // Recovery from abort
                self.ones_count = 0;
                return;
            }

            if self.ones_count == 6 {
                // Flag detected (01111110).
                // We pushed 7 bits into bit_buffer as data: the leading 0 of the
                // flag plus the 6 ones. Remove all 7.
                if self.in_frame {
                    let buf_len = self.bit_buffer.len();
                    let remove = 7.min(buf_len);
                    self.bit_buffer.truncate(buf_len - remove);
                    if !self.bit_buffer.is_empty() {
                        self.try_extract_frame();
                    }
                }
                self.hdlc_start = self.bit_position;
                self.in_frame = true;
                self.bit_buffer.clear();
                self.ones_count = 0;
                return;
            }

            if self.ones_count == 5 {
                // Stuffed zero — the 5 ones were real data (already pushed).
                // Discard this 0.
                self.ones_count = 0;
                return;
            }

            // Normal 0 bit
            self.ones_count = 0;
            if self.in_frame {
                self.bit_buffer.push(0);
            }
        }
    }

    /// Attempt to extract a frame from the accumulated bit buffer.
    fn try_extract_frame(&mut self) {
        // Need at least 16 bits for 2-byte CRC
        if self.bit_buffer.len() < 16 {
            return;
        }

        // Must be a multiple of 8 bits
        let bit_len = self.bit_buffer.len();
        if bit_len % 8 != 0 {
            return;
        }

        let num_bytes = bit_len / 8;
        let mut bytes = Vec::with_capacity(num_bytes);
        for i in 0..num_bytes {
            bytes.push(bits_to_byte_lsb(&self.bit_buffer[i * 8..(i + 1) * 8]));
        }

        // CRC verification: last 2 bytes are FCS (low byte first)
        if bytes.len() < 3 {
            return; // Need at least 1 data byte + 2 CRC bytes
        }

        let data = &bytes[..bytes.len() - 2];
        let fcs_lo = bytes[bytes.len() - 2];
        let fcs_hi = bytes[bytes.len() - 1];
        let received_fcs = u16::from_le_bytes([fcs_lo, fcs_hi]);

        let crc_alg = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let computed_fcs = crc_alg.checksum(data);

        if computed_fcs == received_fcs {
            // Push the full frame including CRC (as the modulator expects)
            let frame = ReceivedFrame {
                bytes,
                start: self.hdlc_start,
                end: self.bit_position,
            };
            if self.fx.is_empty() {
                self.emit_frame(frame);
            } else {
                self.pending_plain.push(frame);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crc::{Crc, CRC_16_IBM_SDLC};

    #[test]
    fn test_byte_to_bits_lsb() {
        // 0x7E = 0111_1110 binary; LSB-first = [0,1,1,1,1,1,1,0]
        assert_eq!(byte_to_bits_lsb(0x7E), [0, 1, 1, 1, 1, 1, 1, 0]);
    }

    #[test]
    fn test_bits_to_byte_lsb_roundtrip() {
        for byte in [0x00u8, 0x7E, 0xFF, 0xA5, 0x1B] {
            let bits = byte_to_bits_lsb(byte);
            assert_eq!(bits_to_byte_lsb(&bits), byte);
        }
    }

    #[test]
    fn test_bit_stuff_no_stuffing_needed() {
        // Alternating bits — never 5 consecutive 1s, so no stuffing.
        let bits = vec![1, 0, 1, 0, 1, 0, 1, 0];
        assert_eq!(bit_stuff(&bits), bits);
    }

    #[test]
    fn test_bit_stuff_inserts_zero_after_five_ones() {
        // Five 1s then a 0: the 0 following the run is data, not a stuff bit.
        // After bit stuffing: [1,1,1,1,1, <stuff 0>, 0]
        let bits = vec![1, 1, 1, 1, 1, 0];
        assert_eq!(bit_stuff(&bits), vec![1, 1, 1, 1, 1, 0, 0]);
    }

    #[test]
    fn test_bit_stuff_six_ones() {
        // Six consecutive 1s: stuff a 0 after the fifth, then continue.
        // [1,1,1,1,1,<stuff 0>,1]
        let bits = vec![1, 1, 1, 1, 1, 1];
        assert_eq!(bit_stuff(&bits), vec![1, 1, 1, 1, 1, 0, 1]);
    }

    #[test]
    fn test_bit_stuff_unstuff_roundtrip() {
        let original = vec![0u8, 1, 1, 1, 1, 1, 1, 0, 1, 0, 1, 1, 1, 1, 1, 0];
        let stuffed = bit_stuff(&original);
        let recovered = bit_unstuff(&stuffed).expect("unstuff should succeed on valid data");
        assert_eq!(recovered, original);
    }

    #[test]
    fn test_bit_unstuff_rejects_six_ones() {
        // Six consecutive 1s without a stuffed zero are invalid.
        let bits = vec![1, 1, 1, 1, 1, 1];
        assert!(bit_unstuff(&bits).is_none());
    }

    #[test]
    fn test_nrzi_encode() {
        // Input:   0   1   0   0   1
        // State:  T→F  F   F→T T→F  F
        // Output:  F   F   T   F    F
        let bits = vec![0u8, 1, 0, 0, 1];
        let expected = vec![false, false, true, false, false];
        assert_eq!(nrzi_encode(&bits), expected);
    }

    #[test]
    fn test_nrzi_encode_decode_roundtrip() {
        let bits: Vec<u8> = vec![1, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1, 0];
        let symbols = nrzi_encode(&bits);
        let decoded = nrzi_decode(&symbols);
        assert_eq!(decoded, bits);
    }

    #[test]
    fn test_modulate_produces_audio() {
        // A minimal frame: 1 data byte.
        let frame = vec![0xABu8];
        let samples = modulate(&frame);

        // Must be non-empty and longer than 1000 samples.
        assert!(
            samples.len() > 1000,
            "Expected > 1000 samples, got {}",
            samples.len()
        );

        // All samples must be in [-1, 1].
        for (i, &s) in samples.iter().enumerate() {
            assert!(
                (-1.0 - 1e-6..=1.0 + 1e-6).contains(&s),
                "Sample {} = {} is out of range",
                i,
                s
            );
        }
    }

    #[test]
    fn test_modulate_length_is_correct() {
        let frame = vec![0x00u8; 4];
        let samples = modulate(&frame);

        // Expected bit count:
        //   preamble: PREAMBLE_FLAGS * 8 bits
        //   frame body: 4 bytes * 8 bits = 32 bits (no stuffing for 0x00)
        //   postamble: POSTAMBLE_FLAGS * 8 bits
        let preamble_bits = PREAMBLE_FLAGS * 8;
        let frame_bits = 4 * 8; // 0x00 has no consecutive 1s, so no stuffing
        let postamble_bits = POSTAMBLE_FLAGS * 8;
        let total_bits = preamble_bits + frame_bits + postamble_bits;
        let expected_samples = total_bits * SAMPLES_PER_SYMBOL as usize;

        assert_eq!(
            samples.len(),
            expected_samples,
            "Expected {} samples, got {}",
            expected_samples,
            samples.len()
        );
    }

    // -----------------------------------------------------------------------
    // Demodulator tests
    // -----------------------------------------------------------------------

    /// Build a test frame with CRC appended.
    fn make_test_frame(data: &[u8]) -> Vec<u8> {
        let crc_alg = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let fcs = crc_alg.checksum(data);
        let mut frame = data.to_vec();
        frame.push(fcs as u8);
        frame.push((fcs >> 8) as u8);
        frame
    }

    #[test]
    fn test_demod_clean_loopback() {
        // A simple AX.25-like frame
        let data = vec![
            0x9C, 0x94, 0x6E, 0xA0, 0x40, 0x40, 0xE0, // dest
            0x9C, 0x6E, 0x98, 0x8A, 0x9A, 0x40, 0x61, // src
            0x03, // control
            0xF0, // PID
            b'H', b'e', b'l', b'l', b'o', // info
        ];
        let frame = make_test_frame(&data);
        let samples = modulate(&frame);

        let mut demod = Demodulator::new();
        demod.process(&samples);
        let frames = demod.take_frames();

        assert_eq!(frames.len(), 1, "Expected 1 frame, got {}", frames.len());
        assert_eq!(frames[0], frame);
    }

    #[test]
    fn test_append_fcs_known_answer() {
        // CRC-16/X.25 check value: FCS("123456789") = 0x906E, sent low byte first.
        let framed = append_fcs(b"123456789");
        assert_eq!(&framed[..9], b"123456789");
        assert_eq!(&framed[9..], &[0x6E, 0x90]);
    }

    #[test]
    fn test_append_fcs_round_trips_through_demodulator() {
        let data = b"N0CALL>APRS:>append_fcs round trip";
        let framed = append_fcs(data);
        let mut demod = Demodulator::new();
        demod.process(&modulate(&framed));
        assert_eq!(demod.take_frames(), vec![framed]);
    }

    #[test]
    fn test_demod_noisy_loopback_15db() {
        let data = vec![
            0x9C, 0x94, 0x6E, 0xA0, 0x40, 0x40, 0xE0, 0x9C, 0x6E, 0x98, 0x8A, 0x9A, 0x40, 0x61,
            0x03, 0xF0, b'T', b'e', b's', b't',
        ];
        let frame = make_test_frame(&data);
        let samples = modulate(&frame);

        // Add AWGN at 15 dB SNR
        let noisy = coppa_channel::awgn_seeded(&samples, 15.0, 42);

        let mut demod = Demodulator::new();
        demod.process(&noisy);
        let frames = demod.take_frames();

        assert_eq!(
            frames.len(),
            1,
            "Expected 1 frame at 15 dB SNR, got {}",
            frames.len()
        );
        assert_eq!(frames[0], frame);
    }

    #[test]
    fn test_demod_multiple_frames() {
        let data1 = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let data2 = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let frame1 = make_test_frame(&data1);
        let frame2 = make_test_frame(&data2);

        let samples1 = modulate(&frame1);
        let samples2 = modulate(&frame2);

        // 50ms silence gap between frames
        let silence_samples = (SAMPLE_RATE as f32 * 0.05) as usize;
        let mut combined = samples1;
        combined.extend(vec![0.0f32; silence_samples]);
        combined.extend(samples2);

        let mut demod = Demodulator::new();
        demod.process(&combined);
        let frames = demod.take_frames();

        assert_eq!(frames.len(), 2, "Expected 2 frames, got {}", frames.len());
        assert_eq!(frames[0], frame1);
        assert_eq!(frames[1], frame2);
    }

    #[test]
    fn test_modulate_contains_mark_and_space_frequencies() {
        // Encode a frame where both marks and spaces must appear.
        // Any non-trivial frame will generate transitions in the bit stream.
        let frame = vec![0x55u8, 0xAA]; // alternating bits — lots of transitions
        let samples = modulate(&frame);

        // Count zero crossings in the audio.  The mark frequency (1200 Hz)
        // produces ~1200 crossings/s and space (2200 Hz) ~2200 crossings/s.
        // At 48 kHz, 1200 Hz → 40 samples/cycle (2 crossings), 2200 Hz → ~21.8.
        // Just verify there are plenty of crossings (both frequencies active).
        let mut crossings = 0usize;
        for i in 1..samples.len() {
            if (samples[i] >= 0.0) != (samples[i - 1] >= 0.0) {
                crossings += 1;
            }
        }

        // At minimum mark frequency (1200 Hz) and 48 kHz SR we expect at least
        // 2 * 1200 / 48000 * samples.len() crossings (≈ 5% of samples).
        let min_crossings = samples.len() / 25;
        assert!(
            crossings >= min_crossings,
            "Too few zero crossings: {} (expected >= {}); audio may be silent",
            crossings,
            min_crossings
        );
    }

    #[test]
    fn fx25_rs_success_still_requires_inner_fcs() {
        let mut frame = append_fcs(b"invalid inner FCS");
        frame[0] ^= 1;
        let flag = byte_to_bits_lsb(0x7e);
        let mut inner = flag.to_vec();
        inner.extend(bit_stuff(
            &frame
                .iter()
                .flat_map(|&b| byte_to_bits_lsb(b))
                .collect::<Vec<_>>(),
        ));
        inner.extend(flag);
        let p = crate::fx25::PROFILES[7];
        let mut i = 0;
        while inner.len() < p.data * 8 {
            inner.push(flag[i % 8]);
            i += 1;
        }
        let block = crate::fx25::encode(
            &inner
                .chunks_exact(8)
                .map(bits_to_byte_lsb)
                .collect::<Vec<_>>(),
            p,
        );
        let mut receiver = BitReceiver::new();
        for byte in p.tag.to_le_bytes().into_iter().chain(block) {
            for bit in byte_to_bits_lsb(byte) {
                receiver.receive_bit(bit);
            }
        }
        assert!(receiver.take_received().is_empty());
    }

    fn stuffed_len(frame: &[u8]) -> u64 {
        let raw: Vec<_> = frame.iter().flat_map(|&b| byte_to_bits_lsb(b)).collect();
        bit_stuff(&raw).len() as u64
    }

    fn assert_near(end: u64, expected: u64) {
        assert!(
            end.abs_diff(expected) <= SAMPLES_PER_SYMBOL as u64,
            "end sample {end}, expected {expected} within one symbol"
        );
    }

    #[test]
    fn bit_receiver_reports_plain_frame_end_sample() {
        const N: u64 = 4800;
        let frame = append_fcs(b"N0CALL>APRS:end sample");
        let mut audio = vec![0.0; N as usize];
        audio.extend(modulate(&frame));
        let mut demod = Demodulator::new();
        demod.process(&audio);
        let received = demod.take_received();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, frame);
        let closing_flag_end =
            N + (PREAMBLE_FLAGS as u64 * 8 + stuffed_len(&frame) + 8) * SAMPLES_PER_SYMBOL as u64;
        assert_near(received[0].1, closing_flag_end + FRONT_END_DELAY);
    }

    #[test]
    fn fx25_frame_end_sample_matches_inner_frame() {
        const N: u64 = 4800;
        let frame = append_fcs(b"N0CALL>APRS:fx25 end sample");
        let mut audio = vec![0.0; N as usize];
        audio.extend(modulate_fx25(&frame).unwrap());
        let mut demod = Demodulator::new();
        demod.process(&audio);
        let received = demod.take_received();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, frame);
        // Preamble, tag, inner opening flag, stuffed frame, inner closing flag.
        let inner_end = N
            + (PREAMBLE_FLAGS as u64 * 8 + 64 + 8 + stuffed_len(&frame) + 8)
                * SAMPLES_PER_SYMBOL as u64;
        assert_near(received[0].1, inner_end + FRONT_END_DELAY);
    }

    fn gain_db(taps: &[f32], hz: f64) -> f64 {
        let taps: Vec<f64> = taps.iter().map(|&t| t as f64).collect();
        20.0 * response(&taps, hz).log10()
    }

    #[test]
    fn bandpass_taps_pass_both_tones() {
        let taps = bandpass_taps();
        assert_eq!(taps.len(), BANDPASS_TAPS);
        for hz in [1200.0, 2200.0] {
            let g = gain_db(&taps, hz);
            assert!(g.abs() <= 1.0, "{hz} Hz gain {g:.2} dB");
        }
        for hz in [300.0, 4000.0] {
            let g = gain_db(&taps, hz);
            assert!(g <= -20.0, "{hz} Hz gain {g:.2} dB");
        }
    }

    #[test]
    fn matched_filter_taps_sum_to_one_and_are_symmetric() {
        let taps = matched_filter_taps();
        assert_eq!(taps.len(), 113);
        let sum: f32 = taps.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "sum {sum}");
        for i in 0..taps.len() {
            assert!((taps[i] - taps[taps.len() - 1 - i]).abs() < 1e-7, "tap {i}");
        }
        assert!(taps.iter().all(|t| t.is_finite()));
    }

    #[test]
    fn mixer_table_matches_direct_oscillator() {
        let table = mixer_table();
        assert_eq!(table.len(), MIXER_PERIOD);
        for k in 0..480 {
            let e = table[k % MIXER_PERIOD];
            let at = |f: f32| std::f64::consts::TAU * f as f64 * k as f64 / SAMPLE_RATE as f64;
            for (got, want) in [
                (e.mark_cos, at(MARK_FREQ).cos()),
                (e.mark_sin, at(MARK_FREQ).sin()),
                (e.space_cos, at(SPACE_FREQ).cos()),
                (e.space_sin, at(SPACE_FREQ).sin()),
            ] {
                assert!((got as f64 - want).abs() <= 1e-6, "k {k}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn non_finite_input_does_not_disable_the_receiver() {
        let frame = append_fcs(b"N0CALL>APRS:after a bad sample");
        let mut demod = Demodulator::new();
        demod.process(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0]);
        demod.process(&vec![0.0; 4800]);
        demod.process(&modulate(&frame));
        demod.process(&vec![0.0; 4800]);
        assert_eq!(demod.take_frames(), vec![frame]);
        let mut level = ToneLevel::default();
        assert_eq!(level.norm(f32::NAN), 0.0);
        assert!(level.peak.is_finite() && level.valley.is_finite());
    }

    #[test]
    fn level_tracker_flushes_to_zero_in_silence() {
        let mut level = ToneLevel::default();
        level.norm(1.0);
        for _ in 0..2_000_000 {
            level.norm(0.0);
        }
        assert_eq!(level.peak, 0.0);
        assert_eq!(level.valley, 0.0);
    }

    #[test]
    fn merge_drops_same_reception_from_another_lane() {
        let mut merge = Merge::default();
        assert!(merge.offer(b"frame", 10_000, 10_000));
        assert!(!merge.offer(b"frame", 10_030, 10_030));
    }

    #[test]
    fn merge_drops_late_fx25_copy_of_same_reception() {
        let mut merge = Merge::default();
        let (end, t) = (50_000, 50_000);
        assert!(merge.offer(b"frame", end, t));
        // The FX.25 lane reports 1.7 s later; an emission-time window would
        // deliver this copy a second time.
        assert!(!merge.offer(b"frame", end + 10, t + 80_000));
    }

    #[test]
    fn merge_keeps_separate_receptions() {
        let mut merge = Merge::default();
        assert!(merge.offer(b"frame", 10_000, 10_000));
        assert!(merge.offer(b"frame", 16_400, 16_400));
    }

    #[test]
    fn merge_keeps_different_frames_with_same_end() {
        let mut merge = Merge::default();
        assert!(merge.offer(b"frame one", 10_000, 10_000));
        assert!(merge.offer(b"frame two", 10_000, 10_000));
    }

    #[test]
    fn merge_history_is_bounded() {
        let mut merge = Merge::default();
        let span = 10 * 60 * SAMPLE_RATE as u64;
        let mut now = 0;
        for i in 0..10_000u64 {
            now = i * span / 10_000;
            assert!(merge.offer(&i.to_le_bytes(), now, now));
        }
        assert!(merge
            .recent
            .iter()
            .all(|&(_, _, at)| now - at <= MERGE_HISTORY_SAMPLES));
        assert!(merge.recent.len() < 10_000);
    }

    #[test]
    fn fx25_exact_stuffed_capacity_boundaries() {
        for byte in [0, 0xff] {
            let mut seen = [false; 4];
            for len in 1..=224 {
                let frame = append_fcs(&vec![byte; len]);
                let raw: Vec<_> = frame.iter().flat_map(|&b| byte_to_bits_lsb(b)).collect();
                let size = (16 + bit_stuff(&raw).len()).div_ceil(8);
                let capacities = [32, 64, 128, 223];
                let fitting = capacities.iter().position(|&capacity| size <= capacity);
                match (fitting, modulate_fx25(&frame)) {
                    (Some(i), Ok(audio)) => {
                        seen[i] = true;
                        assert_eq!(
                            audio.len(),
                            (36 * 8 + 64 + (capacities[i] + 32) * 8 + 2 * 8) * 40
                        );
                        // All-ones bodies exercise extra stuffing; the FCS
                        // and closing flag must survive every size transition.
                        if len % 17 == 0 {
                            let mut receiver = Demodulator::new();
                            receiver.process(&audio);
                            assert_eq!(receiver.take_frames(), vec![frame]);
                        }
                    }
                    (None, Err(Fx25Error::FrameTooLong)) => {}
                    other => panic!("wrong stuffed-capacity result for {len} x {byte}: {other:?}"),
                }
            }
            assert!(seen.into_iter().all(|seen| seen));
        }
    }
}
