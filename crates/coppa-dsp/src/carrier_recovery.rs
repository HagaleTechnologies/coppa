//! BPSK Costas loop for carrier frequency/phase recovery.
//!
//! Strips the carrier from the input signal, outputting baseband I-channel
//! samples. Uses cascaded arm filters to properly reject double-frequency
//! mixer products before the phase error detector.
use std::f32::consts::TAU;

/// BPSK Costas loop tracking carrier phase and frequency.
pub struct CostasLoop {
    phase: f32,
    freq: f32,
    base_freq: f32,
    kp: f32,
    ki: f32,
    i_filt1: f32,
    i_filt2: f32,
    q_filt1: f32,
    q_filt2: f32,
    arm_alpha: f32,
}

impl CostasLoop {
    pub fn new(carrier_freq: f32, sample_rate: f32, loop_bandwidth: f32) -> Self {
        let zeta = std::f32::consts::FRAC_1_SQRT_2;
        let denom = 1.0 + 2.0 * zeta * loop_bandwidth + loop_bandwidth * loop_bandwidth;
        let kp = 4.0 * zeta * loop_bandwidth / denom;
        let ki = 4.0 * loop_bandwidth * loop_bandwidth / denom;

        // Floor at 30 Hz prevents extremely slow arm filters for low carrier frequencies
        let fc = 200.0f32.min(carrier_freq * 0.3).max(30.0);
        let arm_alpha = (TAU * fc / sample_rate).min(0.3);

        Self {
            phase: 0.0,
            freq: 0.0,
            base_freq: TAU * carrier_freq / sample_rate,
            kp,
            ki,
            i_filt1: 0.0,
            i_filt2: 0.0,
            q_filt1: 0.0,
            q_filt2: 0.0,
            arm_alpha,
        }
    }

    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        let mut output = Vec::with_capacity(samples.len());

        for &sample in samples {
            let i_raw = 2.0 * sample * self.phase.cos();
            let q_raw = -2.0 * sample * self.phase.sin();

            self.i_filt1 += self.arm_alpha * (i_raw - self.i_filt1);
            self.i_filt2 += self.arm_alpha * (self.i_filt1 - self.i_filt2);
            self.q_filt1 += self.arm_alpha * (q_raw - self.q_filt1);
            self.q_filt2 += self.arm_alpha * (self.q_filt1 - self.q_filt2);

            let error = self.i_filt2 * self.q_filt2;

            self.freq += self.ki * error;
            self.freq = self.freq.clamp(-0.12, 0.12);

            self.phase += self.base_freq + self.kp * error + self.freq;

            while self.phase > std::f32::consts::PI {
                self.phase -= TAU;
            }
            while self.phase < -std::f32::consts::PI {
                self.phase += TAU;
            }

            output.push(self.i_filt2);
        }

        output
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.freq = 0.0;
        self.i_filt1 = 0.0;
        self.i_filt2 = 0.0;
        self.q_filt1 = 0.0;
        self.q_filt2 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn generate_bpsk_signal(
        bits: &[u8],
        carrier_freq: f32,
        sample_rate: f32,
        sps: usize,
    ) -> Vec<f32> {
        let mut samples = Vec::new();
        let mut global_n = 0usize;
        for &bit in bits {
            let phase_offset = if bit == 0 { 0.0 } else { PI };
            for _ in 0..sps {
                let t = global_n as f32 / sample_rate;
                samples.push((TAU * carrier_freq * t + phase_offset).cos());
                global_n += 1;
            }
        }
        samples
    }

    fn check_demod(
        bits: &[u8],
        baseband: &[f32],
        sps: usize,
        settle_symbols: usize,
    ) -> (usize, usize) {
        let mut correct = 0;
        let mut total = 0;
        for (i, &bit) in bits.iter().enumerate().skip(settle_symbols) {
            let start = i * sps;
            let end = start + sps;
            if end > baseband.len() {
                break;
            }
            let avg: f32 = baseband[start..end].iter().sum::<f32>() / sps as f32;
            let decoded = if avg > 0.0 { 0u8 } else { 1u8 };
            total += 1;
            if decoded == bit {
                correct += 1;
            }
        }
        (correct, total)
    }

    #[test]
    fn test_costas_no_offset() {
        let bits = vec![0, 1, 1, 0, 1, 0, 0, 1, 0, 1];
        let sps = 256;
        let carrier = 1000.0;
        let sr = 8000.0;
        let samples = generate_bpsk_signal(&bits, carrier, sr, sps);

        let mut costas = CostasLoop::new(carrier, sr, 0.02);
        let baseband = costas.process(&samples);

        let (correct, total) = check_demod(&bits, &baseband, sps, 2);
        assert_eq!(
            correct, total,
            "All symbols after settling should be correct"
        );
    }

    #[test]
    fn test_costas_with_freq_offset() {
        let bits = vec![1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 0, 1, 0];
        let sps = 256;
        let sr = 8000.0;
        let carrier = 1000.0;
        let offset_hz = 10.0;

        let samples = generate_bpsk_signal(&bits, carrier + offset_hz, sr, sps);

        let mut costas = CostasLoop::new(carrier, sr, 0.02);
        let baseband = costas.process(&samples);

        let (correct, total) = check_demod(&bits, &baseband, sps, 4);
        assert_eq!(
            correct, total,
            "Should decode all symbols after settling, got {}/{}",
            correct, total
        );
    }

    #[test]
    fn test_costas_with_large_freq_offset() {
        let bits = vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 1];
        let sps = 256;
        let sr = 8000.0;
        let carrier = 1000.0;
        let offset_hz = 30.0;

        let samples = generate_bpsk_signal(&bits, carrier + offset_hz, sr, sps);

        let mut costas = CostasLoop::new(carrier, sr, 0.03);
        let baseband = costas.process(&samples);

        let (correct, total) = check_demod(&bits, &baseband, sps, 6);
        assert!(
            correct as f32 / total as f32 > 0.9,
            "Should decode most symbols, got {}/{}",
            correct,
            total
        );
    }

    #[test]
    fn test_costas_with_phase_offset() {
        let bits = vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0];
        let sps = 256;
        let sr = 8000.0;
        let carrier = 1000.0;
        let phase_shift = PI / 3.0;

        let mut samples = Vec::new();
        let mut global_n = 0usize;
        for &bit in &bits {
            let phase_offset = if bit == 0 { 0.0 } else { PI };
            for _ in 0..sps {
                let t = global_n as f32 / sr;
                samples.push((TAU * carrier * t + phase_offset + phase_shift).cos());
                global_n += 1;
            }
        }

        let mut costas = CostasLoop::new(carrier, sr, 0.02);
        let baseband = costas.process(&samples);

        let (correct, total) = check_demod(&bits, &baseband, sps, 3);
        assert_eq!(
            correct, total,
            "Should handle phase offset, got {}/{}",
            correct, total
        );
    }
}
