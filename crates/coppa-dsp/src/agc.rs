/// Block-adaptive Automatic Gain Control.
///
/// Tracks amplitude changes over time using a leaky integrator with
/// attack/release asymmetry. Suitable for fading channels.
///
/// ## Attack/Release Convention
///
/// `beta_attack` (default 0.6) is the smoothing weight used when signal power
/// is *increasing*, and `beta_release` (default 0.85) is used when power is
/// *decreasing*. A **smaller** beta means **faster** tracking (more weight on
/// the new measurement). So `beta_attack < beta_release` means the AGC responds
/// faster to sudden power increases (attack) than to decreases (release), which
/// prevents clipping on strong signals while avoiding pumping on fades.
pub struct AdaptiveAgc {
    target_level: f32,
    block_size: usize,
    beta_attack: f32,
    beta_release: f32,
    max_gain: f32,
    min_gain: f32,
    power_estimate: Option<f32>,
}

impl AdaptiveAgc {
    pub fn new(target_level: f32, block_size: usize) -> Self {
        Self {
            target_level,
            block_size,
            beta_attack: 0.6,
            beta_release: 0.85,
            max_gain: 10000.0,
            min_gain: 0.0001,
            power_estimate: None,
        }
    }

    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let mut output = Vec::with_capacity(samples.len());

        for chunk in samples.chunks(self.block_size) {
            let block_power = chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32;

            let power_est = match self.power_estimate {
                None => {
                    self.power_estimate = Some(block_power);
                    block_power
                }
                Some(prev) => {
                    let beta = if block_power > prev {
                        self.beta_attack
                    } else {
                        self.beta_release
                    };
                    let new_est = beta * prev + (1.0 - beta) * block_power;
                    self.power_estimate = Some(new_est);
                    new_est
                }
            };

            let rms = power_est.sqrt();
            let gain = if rms > 1e-10 {
                (self.target_level / rms).clamp(self.min_gain, self.max_gain)
            } else {
                1.0
            };

            for &sample in chunk {
                output.push(sample * gain);
            }
        }

        output
    }

    pub fn reset(&mut self) {
        self.power_estimate = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agc_normalizes_quiet_signal() {
        let mut agc = AdaptiveAgc::new(1.0, 64);
        let quiet: Vec<f32> = (0..1024).map(|i| 0.01 * (i as f32 * 0.1).sin()).collect();
        let output = agc.process(&quiet);

        let in_rms = (quiet.iter().map(|x| x * x).sum::<f32>() / quiet.len() as f32).sqrt();
        let out_rms = (output.iter().map(|x| x * x).sum::<f32>() / output.len() as f32).sqrt();
        assert!(
            out_rms > in_rms * 5.0,
            "AGC should amplify quiet signals: in_rms={}, out_rms={}",
            in_rms,
            out_rms
        );
    }

    #[test]
    fn test_agc_normalizes_loud_signal() {
        let mut agc = AdaptiveAgc::new(1.0, 64);
        let loud: Vec<f32> = (0..1024).map(|i| 50.0 * (i as f32 * 0.1).sin()).collect();
        let output = agc.process(&loud);

        let out_rms = (output.iter().map(|x| x * x).sum::<f32>() / output.len() as f32).sqrt();
        assert!(
            out_rms < 3.0,
            "AGC should attenuate loud signals to near target (1.0), got rms={}",
            out_rms
        );
    }

    #[test]
    fn test_agc_tracks_amplitude_change() {
        let mut agc = AdaptiveAgc::new(1.0, 64);

        let mut signal = Vec::new();
        for i in 0..4096 {
            signal.push(10.0 * (i as f32 * 0.5).sin());
        }
        for i in 0..4096 {
            signal.push(0.1 * (i as f32 * 0.5).sin());
        }

        let output = agc.process(&signal);

        let first_rms = (output[3840..4096].iter().map(|x| x * x).sum::<f32>() / 256.0).sqrt();
        let second_rms = (output[7840..8096].iter().map(|x| x * x).sum::<f32>() / 256.0).sqrt();

        let ratio = if first_rms > second_rms {
            first_rms / second_rms
        } else {
            second_rms / first_rms
        };
        assert!(
            ratio < 2.0,
            "AGC should track amplitude change, ratio={}",
            ratio
        );
    }
}
