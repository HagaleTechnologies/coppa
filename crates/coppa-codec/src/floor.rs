//! Production 16-FSK low-rate floor modulation (COP-190, ADR-014).
//!
//! Sixteen orthogonal tones spaced 50 Hz apart, keyed at 50 baud on the
//! workspace's 48 kHz clock. The lowest tone is 600 Hz and the highest
//! 1350 Hz, inside the 300-2700 Hz SSB passband. This module is pure
//! modulation: the version-2 framing, header protection and FEC that use
//! it live in `coppa_protocol::modem::floor`.
//!
//! The design is the measured COP-169/COP-189 16-FSK candidate moved from
//! `coppa-bench` into a production crate; the experimental bench copy stays
//! frozen for measurement continuity.

use num_complex::Complex32;

/// All floor audio uses the workspace's 48 kHz clock.
pub const SAMPLE_RATE: usize = 48_000;
/// 50 baud: one symbol lasts 960 samples, making the 16 tones orthogonal.
pub const SYMBOL_SAMPLES: usize = 960;
/// Tone spacing of the 16-tone alphabet, equal to the baud rate.
pub const TONE_SPACING_HZ: usize = 50;
/// Lowest tone frequency of the alphabet.
pub const LOWEST_TONE_HZ: usize = 600;
/// Tone alphabet size: four bits per symbol.
pub const TONES: usize = 16;
/// Sliding matched-filter hop, one eighth of a symbol: the energy bank
/// evaluates all eight symbol phases.
pub const HOP: usize = SYMBOL_SAMPLES / 8;
/// 450..1500 Hz in 25 Hz steps covers all 16 tones at every CFO hypothesis
/// of the plus-or-minus 150 Hz search grid.
pub const SEARCH_BINS: usize = 43;
/// Matched-filter template length; the 25 Hz grid repeats every two symbols.
const TEMPLATE_SAMPLES: usize = 2 * SYMBOL_SAMPLES;

/// The 16-FSK tone modulator and sliding matched-filter energy bank.
///
/// The oscillator bank is built once and reused for every frame in both
/// directions: [`FloorFsk::tone_waveform`] renders one symbol and
/// [`FloorFsk::energies`] evaluates every 25 Hz search bin at every
/// eighth-symbol phase. Memory is bounded by the bank: 43 complex
/// templates of 1920 samples plus 16 real render templates.
#[derive(Debug)]
pub struct FloorFsk {
    templates: Vec<Vec<Complex32>>,
    tone_real: Vec<Vec<f32>>,
}

impl Default for FloorFsk {
    fn default() -> Self {
        Self::new()
    }
}

impl FloorFsk {
    /// Build the 450..1500 Hz complex search templates in 25 Hz steps and
    /// the 16 real render waveforms.
    #[must_use]
    pub fn new() -> Self {
        let templates: Vec<Vec<Complex32>> = (0..SEARCH_BINS)
            .map(|bin| {
                (0..TEMPLATE_SAMPLES)
                    .map(|n| {
                        Complex32::from_polar(
                            1.0,
                            std::f32::consts::TAU * (450 + 25 * bin) as f32 * n as f32
                                / SAMPLE_RATE as f32,
                        )
                    })
                    .collect()
            })
            .collect();
        let tone_real = (0..TONES)
            .map(|tone| {
                let bin = Self::tone_bin(tone);
                templates[bin][..SYMBOL_SAMPLES]
                    .iter()
                    .map(|z| z.re)
                    .collect()
            })
            .collect();
        Self {
            templates,
            tone_real,
        }
    }

    /// Frequency of tone `tone` of the 16-tone alphabet, in Hz.
    #[must_use]
    pub fn tone_frequency_hz(tone: usize) -> usize {
        LOWEST_TONE_HZ + TONE_SPACING_HZ * tone
    }

    /// Search-bin index of tone `tone`. Consecutive tones sit two 25 Hz bins
    /// apart, so every CFO hypothesis of the search grid maps the whole
    /// alphabet onto the bank by bin offset alone.
    #[must_use]
    pub fn tone_bin(tone: usize) -> usize {
        6 + 2 * tone
    }

    /// One rendered symbol of `tone`: `SYMBOL_SAMPLES` samples of the
    /// unit-amplitude tone.
    #[must_use]
    pub fn tone_waveform(&self, tone: usize) -> &[f32] {
        &self.tone_real[tone]
    }

    /// Sliding matched-filter energies over `audio`, one row per `HOP`
    /// samples, each row holding the squared magnitude of every search bin.
    /// Returns an empty bank below one symbol of audio. Incremental sliding
    /// keeps the cost one pass over the audio plus per-bin updates.
    #[must_use]
    pub fn energies(&self, audio: &[f32]) -> Vec<[f32; SEARCH_BINS]> {
        if audio.len() < SYMBOL_SAMPLES {
            return Vec::new();
        }
        let mut sums = [Complex32::new(0.0, 0.0); SEARCH_BINS];
        for (bin, sum) in sums.iter_mut().enumerate() {
            for (n, &x) in audio[..SYMBOL_SAMPLES].iter().enumerate() {
                *sum += self.templates[bin][n] * x;
            }
        }
        let mut out = Vec::new();
        let mut start = 0;
        loop {
            out.push(sums.map(|z| z.norm_sqr().max(0.0)));
            if start + SYMBOL_SAMPLES + HOP > audio.len() {
                break;
            }
            for (bin, sum) in sums.iter_mut().enumerate() {
                for n in start..start + HOP {
                    *sum += self.templates[bin][(n + SYMBOL_SAMPLES) % TEMPLATE_SAMPLES]
                        * audio[n + SYMBOL_SAMPLES]
                        - self.templates[bin][n % TEMPLATE_SAMPLES] * audio[n];
                }
            }
            start += HOP;
        }
        out
    }
}
