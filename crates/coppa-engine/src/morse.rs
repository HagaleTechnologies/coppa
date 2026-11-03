//! Morse (CW) keying for station identification (COP-84, H-038).
//!
//! Timing is PARIS: one unit = 1.2 / WPM seconds; dit 1, dah 3, gap inside a
//! character 1, between characters 3, between words 7. Each key-down run
//! gets a raised-cosine rise and fall inside its own duration, so timing is
//! exact and key clicks stay near the tone.

use coppa_codec::ofdm::coppa_modem::TX_PEAK;

/// Audio tone the daemon keys its CW ID on (Hz), inside every SSB passband.
pub const CW_ID_TONE_HZ: f32 = 1000.0;
/// Raised-cosine rise and fall time of each key-down run (seconds).
pub const CW_EDGE_SECONDS: f32 = 0.005;
/// Silent units ahead of the first element: one word gap, so the ID is heard
/// apart from whatever audio it follows.
pub const LEAD_IN_UNITS: usize = 7;
/// Longest text [`keying`] accepts, in characters.
pub const MAX_TEXT_CHARS: usize = 32;
/// Generator bounds, not a regulatory range (the daemon enforces 10-20).
pub const MIN_WPM: u32 = 1;
/// Upper generator bound on speed (WPM); see [`MIN_WPM`].
pub const MAX_WPM: u32 = 60;

/// Highest sample rate [`keyed_tone`] accepts, so no input can allocate
/// without bound.
const MAX_SAMPLE_RATE: u32 = 384_000;

/// Why text could not be keyed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MorseError {
    /// The text is empty.
    #[error("no text to key")]
    Empty,
    /// The text has more than [`MAX_TEXT_CHARS`] characters.
    #[error("text is {len} characters; at most {max} can be keyed")]
    TooLong {
        /// Characters in the text.
        len: usize,
        /// The most that can be keyed, [`MAX_TEXT_CHARS`].
        max: usize,
    },
    /// A character with no Morse pattern (only `A-Z`, `0-9` and `/` key).
    #[error("character {0:?} has no Morse pattern (A-Z, 0-9 and / only)")]
    UnsupportedCharacter(char),
    /// A speed outside [`MIN_WPM`]..=[`MAX_WPM`].
    #[error("speed {0} WPM is outside {min}-{max} WPM", min = MIN_WPM, max = MAX_WPM)]
    Speed(u32),
    /// A sample rate of 0, above 384 kHz, or too low to give one sample per unit.
    #[error("sample rate {0} Hz cannot carry Morse keying")]
    SampleRate(u32),
}

/// The ITU-R M.1677-1 pattern for `ch` (`.` dit, `-` dah): upper-case `A-Z`,
/// `0-9` and `/`, the characters a callsign keys. `None` for anything else,
/// including `-` (an SSID is not keyed) and lower case.
pub fn pattern(ch: char) -> Option<&'static str> {
    Some(match ch {
        'A' => ".-",
        'B' => "-...",
        'C' => "-.-.",
        'D' => "-..",
        'E' => ".",
        'F' => "..-.",
        'G' => "--.",
        'H' => "....",
        'I' => "..",
        'J' => ".---",
        'K' => "-.-",
        'L' => ".-..",
        'M' => "--",
        'N' => "-.",
        'O' => "---",
        'P' => ".--.",
        'Q' => "--.-",
        'R' => ".-.",
        'S' => "...",
        'T' => "-",
        'U' => "..-",
        'V' => "...-",
        'W' => ".--",
        'X' => "-..-",
        'Y' => "-.--",
        'Z' => "--..",
        '0' => "-----",
        '1' => ".----",
        '2' => "..---",
        '3' => "...--",
        '4' => "....-",
        '5' => ".....",
        '6' => "-....",
        '7' => "--...",
        '8' => "---..",
        '9' => "----.",
        '/' => "-..-.",
        _ => return None,
    })
}

/// The part of `callsign` a CW ID keys: upper-cased and cut at the first `-`,
/// so `N1CALL-10` keys `N1CALL` (an SSID is a network address, not part of
/// the licensed call) and a `/` designator is kept.
pub fn cw_id_text(callsign: &str) -> String {
    callsign
        .split_once('-')
        .map_or(callsign, |(call, _ssid)| call)
        .to_ascii_uppercase()
}

/// Check `text` before anything is allocated for it.
fn validate_text(text: &str) -> Result<(), MorseError> {
    let len = text.chars().count();
    if len == 0 {
        return Err(MorseError::Empty);
    }
    if len > MAX_TEXT_CHARS {
        return Err(MorseError::TooLong {
            len,
            max: MAX_TEXT_CHARS,
        });
    }
    match text.chars().find(|&c| pattern(c).is_none()) {
        Some(c) => Err(MorseError::UnsupportedCharacter(c)),
        None => Ok(()),
    }
}

/// One entry per Morse unit, `true` while the key is down: [`LEAD_IN_UNITS`]
/// key-up units, then each character's elements. No trailing gap follows the
/// last element.
pub fn keying(text: &str) -> Result<Vec<bool>, MorseError> {
    validate_text(text)?;
    let mut units = vec![false; LEAD_IN_UNITS];
    for (i, ch) in text.chars().enumerate() {
        if i > 0 {
            units.extend([false; 3]);
        }
        // validate_text has already checked every character has a pattern.
        let elements = pattern(ch).ok_or(MorseError::UnsupportedCharacter(ch))?;
        for (j, element) in elements.chars().enumerate() {
            if j > 0 {
                units.push(false);
            }
            let length = if element == '-' { 3 } else { 1 };
            units.extend(std::iter::repeat_n(true, length));
        }
    }
    Ok(units)
}

/// `text` keyed as a `tone_hz` sine at `wpm`, sampled at `sample_rate`.
///
/// Every unit is `round(sample_rate * 1.2 / wpm)` samples, so the length is
/// exactly `keying(text).len()` times that. The tone's phase runs from sample
/// 0; key-up samples are exactly 0.0; each key-down run rises and falls over
/// [`CW_EDGE_SECONDS`] (raised cosine, at most half the run), starting and
/// ending at 0.0. Peak-normalised to [`TX_PEAK`], like `CoppaCore::tune_tone`.
pub fn keyed_tone(
    text: &str,
    wpm: u32,
    tone_hz: f32,
    sample_rate: u32,
) -> Result<Vec<f32>, MorseError> {
    if !(MIN_WPM..=MAX_WPM).contains(&wpm) {
        return Err(MorseError::Speed(wpm));
    }
    if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
        return Err(MorseError::SampleRate(sample_rate));
    }
    let unit_len = (f64::from(sample_rate) * 1.2 / f64::from(wpm)).round() as usize;
    if unit_len == 0 {
        return Err(MorseError::SampleRate(sample_rate));
    }
    let keys = keying(text)?;

    let rate = f64::from(sample_rate);
    let tone = f64::from(tone_hz);
    let edge_len = (f64::from(CW_EDGE_SECONDS) * rate).round() as usize;
    let mut samples = vec![0.0f32; keys.len() * unit_len];

    let mut unit = 0;
    while unit < keys.len() {
        if !keys[unit] {
            unit += 1;
            continue;
        }
        let first = unit;
        while unit < keys.len() && keys[unit] {
            unit += 1;
        }
        let start = first * unit_len;
        let run = &mut samples[start..unit * unit_len];
        let edge = edge_len.min(run.len() / 2);
        let last = run.len() - 1;
        for (k, s) in run.iter_mut().enumerate() {
            let from_edge = k.min(last - k);
            let envelope = if from_edge < edge {
                0.5 * (1.0 - (std::f64::consts::PI * from_edge as f64 / edge as f64).cos())
            } else {
                1.0
            };
            // Phase from the cycle count's fractional part keeps it exact
            // over a long buffer.
            let cycles = (tone * (start + k) as f64 / rate).fract();
            *s = (envelope * (std::f64::consts::TAU * cycles).sin()) as f32;
        }
    }

    let peak = samples.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    if peak > 1e-12 {
        let gain = TX_PEAK / peak;
        for s in &mut samples {
            *s *= gain;
        }
    }
    Ok(samples)
}

/// What the daemon keys: `keyed_tone(&cw_id_text(callsign), wpm, CW_ID_TONE_HZ, sample_rate)`.
pub fn cw_id_samples(callsign: &str, wpm: u32, sample_rate: u32) -> Result<Vec<f32>, MorseError> {
    keyed_tone(&cw_id_text(callsign), wpm, CW_ID_TONE_HZ, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_dsp::fft::FftProcessor;
    use num_complex::Complex32;

    const RATE: u32 = 48_000;

    /// Run-length encode the keying: `(key_down, units)` pairs.
    fn runs(keys: &[bool]) -> Vec<(bool, usize)> {
        let mut out: Vec<(bool, usize)> = Vec::new();
        for &k in keys {
            match out.last_mut() {
                Some((down, n)) if *down == k => *n += 1,
                _ => out.push((k, 1)),
            }
        }
        out
    }

    fn unit_len(wpm: u32) -> usize {
        (f64::from(RATE) * 1.2 / f64::from(wpm)).round() as usize
    }

    /// Key-down sample ranges of `text` at `wpm`.
    fn key_down_ranges(text: &str, wpm: u32) -> Vec<std::ops::Range<usize>> {
        let n = unit_len(wpm);
        let mut ranges = Vec::new();
        let mut unit = 0;
        for (down, len) in runs(&keying(text).unwrap()) {
            if down {
                ranges.push(unit * n..(unit + len) * n);
            }
            unit += len;
        }
        ranges
    }

    /// Test-only decoder: key state per unit from the RMS of the middle half
    /// of the unit, then runs back through the pattern table.
    fn decode(samples: &[f32], wpm: u32) -> String {
        let n = unit_len(wpm);
        assert_eq!(samples.len() % n, 0);
        let keys: Vec<bool> = samples
            .chunks(n)
            .map(|unit| {
                let middle = &unit[n / 4..3 * n / 4];
                let rms = (middle.iter().map(|s| s * s).sum::<f32>() / middle.len() as f32).sqrt();
                rms > TX_PEAK / 4.0
            })
            .collect();
        let table: Vec<(char, &str)> = ('A'..='Z')
            .chain('0'..='9')
            .chain(['/'])
            .map(|c| (c, pattern(c).unwrap()))
            .collect();
        let mut text = String::new();
        let mut current = String::new();
        let flush = |current: &mut String, text: &mut String| {
            if !current.is_empty() {
                let ch = table
                    .iter()
                    .find(|(_, p)| *p == current.as_str())
                    .map(|(c, _)| *c)
                    .unwrap_or('?');
                text.push(ch);
                current.clear();
            }
        };
        for (down, len) in runs(&keys) {
            match (down, len) {
                (true, 1) => current.push('.'),
                (true, 3) => current.push('-'),
                (true, _) => current.push('?'),
                (false, 1) => {}
                (false, _) => flush(&mut current, &mut text),
            }
        }
        flush(&mut current, &mut text);
        text
    }

    #[test]
    fn cop84_patterns_cover_the_callsign_alphabet() {
        for c in ('A'..='Z').chain('0'..='9').chain(['/']) {
            assert!(pattern(c).is_some(), "{c:?} has no pattern");
        }
        assert_eq!(pattern('N'), Some("-."));
        assert_eq!(pattern('1'), Some(".----"));
        assert_eq!(pattern('0'), Some("-----"));
        assert_eq!(pattern('/'), Some("-..-."));
        assert_eq!(pattern('-'), None);
        assert_eq!(pattern(' '), None);
        assert_eq!(pattern('!'), None);
    }

    #[test]
    fn cop84_paris_is_fifty_units_and_three_seconds_at_20_wpm() {
        let keys = keying("PARIS").unwrap();
        assert_eq!(keys.len(), 50);
        assert!(keys[..LEAD_IN_UNITS].iter().all(|&k| !k));
        let tone = keyed_tone("PARIS", 20, CW_ID_TONE_HZ, RATE).unwrap();
        assert_eq!(tone.len(), 50 * 2880);
        assert_eq!(tone.len(), 3 * RATE as usize);
    }

    #[test]
    fn cop84_unit_counts_match_the_itu_timing() {
        assert_eq!(keying("N1CALL").unwrap().len(), 78);
        assert_eq!(keying("VE3ABCD").unwrap().len(), 80);
        assert_eq!(keying("N1CALL/P").unwrap().len(), 108);
        assert_eq!(keying("00000000").unwrap().len(), 180);

        // N -.  1 .----  C -.-.  A .-  L .-..  L .-..
        let expected: Vec<(bool, usize)> = vec![
            (false, 7),
            (true, 3),
            (false, 1),
            (true, 1),
            (false, 3),
            (true, 1),
            (false, 1),
            (true, 3),
            (false, 1),
            (true, 3),
            (false, 1),
            (true, 3),
            (false, 1),
            (true, 3),
            (false, 3),
            (true, 3),
            (false, 1),
            (true, 1),
            (false, 1),
            (true, 3),
            (false, 1),
            (true, 1),
            (false, 3),
            (true, 1),
            (false, 1),
            (true, 3),
            (false, 3),
            (true, 1),
            (false, 1),
            (true, 3),
            (false, 1),
            (true, 1),
            (false, 1),
            (true, 1),
            (false, 3),
            (true, 1),
            (false, 1),
            (true, 3),
            (false, 1),
            (true, 1),
            (false, 1),
            (true, 1),
        ];
        assert_eq!(runs(&keying("N1CALL").unwrap()), expected);
    }

    #[test]
    fn cop84_cw_id_text_drops_the_ssid_and_keeps_a_portable_suffix() {
        assert_eq!(cw_id_text("N1CALL-10"), "N1CALL");
        assert_eq!(cw_id_text("N1CALL/P-2"), "N1CALL/P");
        assert_eq!(cw_id_text("VE3/N1CALL"), "VE3/N1CALL");
        assert_eq!(cw_id_text("n1call"), "N1CALL");
        assert_eq!(
            cw_id_samples("N1CALL-10", 20, RATE).unwrap(),
            cw_id_samples("N1CALL", 20, RATE).unwrap()
        );
        assert_eq!(cw_id_samples("N1CALL-10", 20, RATE).unwrap().len(), 224_640);
    }

    #[test]
    fn cop84_tone_peaks_at_tx_peak_and_is_silent_between_elements() {
        for wpm in [10, 20] {
            let tone = keyed_tone("N1CALL", wpm, CW_ID_TONE_HZ, RATE).unwrap();
            let peak = tone.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
            assert!((peak - TX_PEAK).abs() < 1e-4, "peak {peak} at {wpm} WPM");

            let n = unit_len(wpm);
            for (unit, &down) in keying("N1CALL").unwrap().iter().enumerate() {
                if !down {
                    assert!(
                        tone[unit * n..(unit + 1) * n].iter().all(|&s| s == 0.0),
                        "key-up unit {unit} is not silent at {wpm} WPM"
                    );
                }
            }
        }
    }

    #[test]
    fn cop84_each_element_starts_and_ends_at_zero_with_5ms_edges() {
        let edge = (CW_EDGE_SECONDS * RATE as f32).round() as usize;
        assert_eq!(edge, 240);
        let tone = keyed_tone("N1CALL", 20, CW_ID_TONE_HZ, RATE).unwrap();
        let ranges = key_down_ranges("N1CALL", 20);
        assert_eq!(ranges.len(), 21);
        for range in ranges {
            let run = &tone[range.clone()];
            assert_eq!(run[0], 0.0, "run {range:?} does not start at zero");
            let rise_peak = run[..=edge].iter().fold(0.0f32, |m, &v| m.max(v.abs()));
            assert!(
                rise_peak >= 0.99 * TX_PEAK,
                "run {range:?} reaches only {rise_peak} within 5 ms"
            );
            let end = run[run.len() - 1];
            assert!(end.abs() < 1e-3, "run {range:?} ends at {end}");
        }
    }

    #[test]
    fn cop84_key_clicks_stay_within_500_hz_of_the_tone() {
        for wpm in [10, 20] {
            let tone = keyed_tone("N1CALL", wpm, CW_ID_TONE_HZ, RATE).unwrap();
            let size = tone.len().next_power_of_two();
            let mut input: Vec<Complex32> = tone.iter().map(|&s| Complex32::new(s, 0.0)).collect();
            input.resize(size, Complex32::new(0.0, 0.0));
            let spectrum = FftProcessor::new(size).forward(&input);

            let mut total = 0.0f64;
            let mut outside = 0.0f64;
            for (bin, c) in spectrum.iter().enumerate().take(size / 2 + 1) {
                let energy = f64::from(c.norm_sqr());
                let hz = bin as f64 * f64::from(RATE) / size as f64;
                total += energy;
                if (hz - f64::from(CW_ID_TONE_HZ)).abs() > 500.0 {
                    outside += energy;
                }
            }
            let percent = 100.0 * outside / total;
            assert!(
                percent < 0.001,
                "{percent:.5}% of the energy lies outside 1000 +/- 500 Hz at {wpm} WPM"
            );
        }
    }

    #[test]
    fn cop84_round_trip_decodes_the_callsign_at_every_allowed_speed() {
        for wpm in 10..=20 {
            for text in ["N1CALL", "VE3/N1CALL", "K0Z"] {
                let tone = keyed_tone(text, wpm, CW_ID_TONE_HZ, RATE).unwrap();
                assert_eq!(decode(&tone, wpm), text, "{text} at {wpm} WPM");
            }
        }
    }

    #[test]
    fn cop84_rejects_bad_input_without_allocating() {
        let hz = CW_ID_TONE_HZ;
        assert_eq!(keyed_tone("", 20, hz, RATE), Err(MorseError::Empty));
        assert_eq!(
            keyed_tone(&"E".repeat(33), 20, hz, RATE),
            Err(MorseError::TooLong { len: 33, max: 32 })
        );
        assert!(keyed_tone(&"E".repeat(32), 20, hz, RATE).is_ok());
        assert_eq!(
            keyed_tone("N1-CALL", 20, hz, RATE),
            Err(MorseError::UnsupportedCharacter('-'))
        );
        assert_eq!(keyed_tone("N1CALL", 0, hz, RATE), Err(MorseError::Speed(0)));
        assert_eq!(
            keyed_tone("N1CALL", 61, hz, RATE),
            Err(MorseError::Speed(61))
        );
        assert_eq!(
            keyed_tone("N1CALL", 20, hz, 0),
            Err(MorseError::SampleRate(0))
        );
        assert_eq!(
            keyed_tone("N1CALL", 1, hz, u32::MAX),
            Err(MorseError::SampleRate(u32::MAX))
        );
        assert_eq!(keying(""), Err(MorseError::Empty));
        assert_eq!(keying("n1call"), Err(MorseError::UnsupportedCharacter('n')));
        assert_eq!(
            MorseError::Speed(61).to_string(),
            "speed 61 WPM is outside 1-60 WPM"
        );
    }
}
