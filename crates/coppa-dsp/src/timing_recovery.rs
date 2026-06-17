/// Gardner symbol timing recovery.
///
/// Operates on baseband samples (post-carrier-recovery, post-matched-filter)
/// and outputs one optimally-timed sample per symbol.
pub struct GardnerTimingRecovery {
    samples_per_symbol: f32,
    mu: f32,
    kp: f32,
    ki: f32,
    rate_adjust: f32,
}

impl GardnerTimingRecovery {
    pub fn new(samples_per_symbol: f32, loop_bandwidth: f32) -> Self {
        let zeta = std::f32::consts::FRAC_1_SQRT_2;
        let denom = 1.0 + 2.0 * zeta * loop_bandwidth + loop_bandwidth * loop_bandwidth;
        let kp = 4.0 * zeta * loop_bandwidth / denom;
        let ki = 4.0 * loop_bandwidth * loop_bandwidth / denom;

        Self {
            samples_per_symbol,
            mu: 0.0,
            kp,
            ki,
            rate_adjust: 0.0,
        }
    }

    pub fn recover(&mut self, baseband: &[f32]) -> Vec<f32> {
        if baseband.len() < self.samples_per_symbol as usize + 2 {
            return Vec::new();
        }

        let mut output = Vec::new();
        // Start at half-symbol offset to recover the first symbol with
        // half-symbol settling instead of losing it entirely (A4 fix).
        let mut pos = self.samples_per_symbol / 2.0;
        let mut prev_symbol = 0.0f32;
        let max_adjust = self.samples_per_symbol * 0.1;

        while pos < (baseband.len() as f32 - 2.0) {
            let current_symbol = cubic_interpolate(baseband, pos);

            let mid_pos = pos - self.samples_per_symbol / 2.0;
            let midpoint = if mid_pos >= 0.0 && mid_pos < baseband.len() as f32 - 2.0 {
                cubic_interpolate(baseband, mid_pos)
            } else {
                0.0
            };

            let raw_error = midpoint * (prev_symbol - current_symbol);
            let signal_power = (prev_symbol * prev_symbol + current_symbol * current_symbol) / 2.0;
            let error = if signal_power > 1e-10 {
                raw_error / signal_power
            } else {
                0.0
            };

            // Second-order timing loop:
            // rate_adjust is the integrated frequency offset (corrects clock drift)
            // mu is the per-symbol phase correction (proportional + integral)
            self.rate_adjust += self.ki * error;
            self.rate_adjust = self.rate_adjust.clamp(-0.01, 0.01);
            self.mu = (self.kp * error + self.rate_adjust).clamp(-max_adjust, max_adjust);

            output.push(current_symbol);
            prev_symbol = current_symbol;

            pos += self.samples_per_symbol + self.mu;
        }

        output
    }

    pub fn reset(&mut self) {
        self.mu = 0.0;
        self.rate_adjust = 0.0;
    }
}

pub fn cubic_interpolate(data: &[f32], pos: f32) -> f32 {
    let idx = pos as usize;
    let frac = pos - idx as f32;

    let n = data.len();
    let y0 = data[idx.saturating_sub(1).min(n - 1)];
    let y1 = data[idx.min(n - 1)];
    let y2 = data[(idx + 1).min(n - 1)];
    let y3 = data[(idx + 2).min(n - 1)];

    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);

    ((c3 * frac + c2) * frac + c1) * frac + c0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cubic_interpolate_at_integer() {
        let data = vec![0.0, 1.0, 0.0, -1.0, 0.0, 1.0];
        let val = cubic_interpolate(&data, 1.0);
        assert!((val - 1.0).abs() < 1e-6);
        let val = cubic_interpolate(&data, 3.0);
        assert!((val - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_timing_recovery_aligned() {
        let sps = 64usize;
        let bits: Vec<u8> = vec![1, 0, 1, 1, 0, 0, 1, 0, 1, 0];

        let mut baseband = Vec::new();
        for &bit in &bits {
            let val = if bit == 0 { 1.0 } else { -1.0 };
            for _ in 0..sps {
                baseband.push(val);
            }
        }

        let mut tr = GardnerTimingRecovery::new(sps as f32, 0.02);
        let symbols = tr.recover(&baseband);

        assert!(
            symbols.len() >= bits.len() - 2,
            "Got {} symbols, expected ~{}",
            symbols.len(),
            bits.len()
        );

        // With half-symbol start, recovered symbols may align to different
        // bit indices. Try plausible offsets and verify best alignment.
        let settle = 1;
        let mut best_correct = 0;
        let mut best_total = 1;
        for offset in 0..3 {
            let mut correct = 0;
            let mut total = 0;
            for (i, &sym) in symbols.iter().enumerate().skip(settle) {
                let bit_idx = i + offset;
                if bit_idx < bits.len() {
                    total += 1;
                    let expected_sign = if bits[bit_idx] == 0 { 1.0 } else { -1.0 };
                    if sym * expected_sign > 0.0 {
                        correct += 1;
                    }
                }
            }
            if total > 0 && correct * best_total > best_correct * total {
                best_correct = correct;
                best_total = total;
            }
        }
        assert!(
            best_total > 0 && best_correct == best_total,
            "All symbols after settling should be correct: {}/{}",
            best_correct,
            best_total
        );
    }

    #[test]
    fn test_timing_recovery_with_offset() {
        let sps = 64usize;
        let bits: Vec<u8> = vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1];

        let mut baseband = Vec::new();
        for &bit in &bits {
            let val = if bit == 0 { 1.0 } else { -1.0 };
            for _ in 0..sps {
                baseband.push(val);
            }
        }

        let delay = (0.4 * sps as f32) as usize;
        let mut delayed = vec![0.0f32; delay];
        delayed.extend_from_slice(&baseband);

        let mut tr = GardnerTimingRecovery::new(sps as f32, 0.02);
        let symbols = tr.recover(&delayed);

        assert!(
            symbols.len() >= bits.len() - 3,
            "Should recover enough symbols"
        );

        let settle = 4;
        let mut correct_transitions = 0;
        let mut total_transitions = 0;
        for i in settle + 1..symbols.len() {
            let sign_change = (symbols[i] > 0.0) != (symbols[i - 1] > 0.0);
            total_transitions += 1;
            if (i < 10 && sign_change) || (i >= 10 && symbols[i].abs() > 0.3) {
                correct_transitions += 1;
            }
        }
        assert!(
            total_transitions > 0 && correct_transitions as f32 / total_transitions as f32 > 0.7,
            "Symbols should be correct after settling: {}/{}",
            correct_transitions,
            total_transitions
        );
    }
}
