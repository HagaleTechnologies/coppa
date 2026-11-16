//! Independent audio-path measurements of the Watterson presets.
use std::f64::consts::{LN_2, PI};
use std::fs;
use std::path::Path;

use coppa_channel::persistent::{PersistentChannel, SnrProfile};
use coppa_channel::watterson::{WattersonConfig, WattersonPreset};

pub const HEADER: &str = "preset,sample_rate,seed,delay_duration_s,doppler_duration_s,delay_set_ms,delay_measured_ms,path_power_ratio_db,spread_set_hz,spread_measured_hz,noise_ref_error_db,within_tolerance";

#[derive(Debug, Clone)]
pub struct Measurement {
    pub preset: &'static str,
    pub sample_rate: u32,
    pub seed: u64,
    pub delay_duration_s: u32,
    pub doppler_duration_s: u32,
    pub delay_set_ms: f64,
    pub delay_measured_ms: f64,
    pub path_power_ratio_db: f64,
    pub spread_set_hz: f64,
    pub spread_measured_hz: f64,
    pub noise_ref_error_db: f64,
    pub within_tolerance: bool,
}

impl Measurement {
    pub fn to_csv(&self) -> String {
        format!(
            "{},{},0x{:08x},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{}",
            self.preset,
            self.sample_rate,
            self.seed,
            self.delay_duration_s,
            self.doppler_duration_s,
            self.delay_set_ms,
            self.delay_measured_ms,
            self.path_power_ratio_db,
            self.spread_set_hz,
            self.spread_measured_hz,
            self.noise_ref_error_db,
            self.within_tolerance
        )
    }
}

fn preset(index: usize) -> (&'static str, WattersonPreset) {
    match index {
        0 => ("good", WattersonPreset::Good),
        1 => ("moderate", WattersonPreset::Moderate),
        _ => ("poor", WattersonPreset::Poor),
    }
}

/// Measure the two tap peaks from a sparse, known audio probe. The spacing
/// exceeds the channel's Hilbert filter and the maximum preset delay.
fn tap_measurement(
    cfg: &WattersonConfig,
    sample_rate: u32,
    seed: u64,
    duration_s: u32,
) -> (f64, f64) {
    let sr = sample_rate as usize;
    let spacing = sr.max(2048);
    let delay = (cfg.taps[1].delay_s * sample_rate as f32).round() as usize;
    let mut channel = PersistentChannel::new(sample_rate as f32, Some(cfg), None, 1.0, seed, 0);
    let latency = channel.latency_samples();
    let blocks = duration_s as usize;
    let mut powers = vec![0.0f64; (delay + 3).max(4)];
    for _ in 0..blocks {
        let mut input = vec![0.0; spacing];
        input[0] = 0.2;
        let output = channel.process(&input);
        for (lag, p) in powers.iter_mut().enumerate() {
            *p += f64::from(output[latency + lag]).powi(2);
        }
    }
    let direct = powers[0].max(1e-30);
    let second_lag = (1..powers.len())
        .max_by(|&a, &b| powers[a].total_cmp(&powers[b]))
        .unwrap_or(1);
    (
        second_lag as f64 * 1000.0 / f64::from(sample_rate),
        10.0 * (powers[second_lag] / direct).log10(),
    )
}

/// Demodulate a 1500 Hz audio tone to 100 complex channel estimates per second.
/// The magnitude of their lagged correlation gives the Gaussian Doppler sigma.
fn doppler_measurement(cfg: &WattersonConfig, sample_rate: u32, seed: u64, duration_s: u32) -> f64 {
    let sr = sample_rate as usize;
    let decimation = (sr / 100).max(1);
    let rate = sample_rate as f64 / decimation as f64;
    let mut channel = PersistentChannel::new(sample_rate as f32, Some(cfg), None, 1.0, seed, 0);
    let sigma_set = f64::from(cfg.doppler_sigma_hz());
    let lag = ((LN_2.sqrt() / ((2.0f64).sqrt() * PI * sigma_set)) * rate)
        .round()
        .max(1.0) as usize;
    let mut samples: Vec<(f64, f64)> = Vec::with_capacity(duration_s as usize * 100);
    let mut clock = 0usize;
    let mut re = 0.0;
    let mut im = 0.0;
    for _ in 0..duration_s {
        let input: Vec<f32> = (0..sr)
            .map(|j| (2.0 * PI * 1500.0 * (clock + j) as f64 / f64::from(sample_rate)).cos() as f32)
            .collect();
        let output = channel.process(&input);
        for (j, &value) in output.iter().enumerate() {
            let phase = 2.0 * PI * 1500.0 * (clock + j) as f64 / f64::from(sample_rate);
            re += f64::from(value) * phase.cos();
            im -= f64::from(value) * phase.sin();
            if (clock + j + 1) % decimation == 0 {
                samples.push((re / decimation as f64, im / decimation as f64));
                re = 0.0;
                im = 0.0;
            }
        }
        clock += sr;
    }
    estimate_spread(&samples, lag, rate)
}

fn estimate_spread(samples: &[(f64, f64)], lag: usize, rate: f64) -> f64 {
    // Ignore filter start-up. Complex correlation removes fixed carrier phase.
    let warmup = rate.ceil() as usize;
    let usable = &samples[warmup.min(samples.len())..];
    if usable.len() <= lag {
        return f64::NAN;
    }
    let mut correlation_re = 0.0;
    let mut correlation_im = 0.0;
    let mut power = 0.0;
    for i in lag..usable.len() {
        let (a, b) = usable[i];
        let (c, d) = usable[i - lag];
        correlation_re += a * c + b * d;
        correlation_im += b * c - a * d;
        power += a * a + b * b;
    }
    let r = correlation_re.hypot(correlation_im) / power.max(1e-30);
    let tau = lag as f64 / rate;
    2.0 * (-r.clamp(1e-12, 1.0).ln() / (2.0 * PI * PI * tau * tau)).sqrt()
}

fn noise_measurement(sample_rate: u32, seed: u64) -> f64 {
    let mut channel = PersistentChannel::new(
        sample_rate as f32,
        None,
        Some(SnrProfile::constant(0.0)),
        1.0,
        seed,
        0,
    );
    let mut sum = 0.0;
    let mut count = 0usize;
    for _ in 0..10 {
        for value in channel.process(&vec![0.0; sample_rate as usize]) {
            sum += f64::from(value).powi(2);
            count += 1;
        }
    }
    let expected = f64::from(sample_rate) / 2.0 / 3000.0;
    10.0 * (sum / count as f64 / expected).log10()
}

pub fn measure(
    index: usize,
    sample_rate: u32,
    delay_duration_s: u32,
    doppler_duration_s: u32,
) -> Measurement {
    let (name, choice) = preset(index);
    let cfg = choice.config();
    let seed = 0x17f0_0000 + index as u64;
    let (delay_measured_ms, path_power_ratio_db) =
        tap_measurement(&cfg, sample_rate, seed, delay_duration_s);
    let spread_measured_hz = doppler_measurement(&cfg, sample_rate, seed, doppler_duration_s);
    let noise_ref_error_db = noise_measurement(sample_rate, seed);
    let delay_set_ms = f64::from(cfg.taps[1].delay_s) * 1000.0;
    let spread_set_hz = f64::from(cfg.doppler_spread_hz);
    let within_tolerance = (delay_measured_ms - delay_set_ms).abs()
        <= 1000.0 / f64::from(sample_rate)
        && path_power_ratio_db.abs() <= 1.5
        && (spread_measured_hz / spread_set_hz - 1.0).abs() <= 0.2
        && noise_ref_error_db.abs() <= 0.2;
    Measurement {
        preset: name,
        sample_rate,
        seed,
        delay_duration_s,
        doppler_duration_s,
        delay_set_ms,
        delay_measured_ms,
        path_power_ratio_db,
        spread_set_hz,
        spread_measured_hz,
        noise_ref_error_db,
        within_tolerance,
    }
}

pub fn read_csv(path: &Path) -> anyhow::Result<Vec<Measurement>> {
    let content = fs::read_to_string(path)?;
    let mut lines = content.lines();
    anyhow::ensure!(lines.next() == Some(HEADER), "invalid channel-check header");
    lines
        .map(|line| {
            let fields: Vec<_> = line.split(',').collect();
            anyhow::ensure!(fields.len() == 12, "invalid channel-check row: {line}");
            let parse = |i: usize| -> anyhow::Result<f64> { Ok(fields[i].parse()?) };
            Ok(Measurement {
                preset: match fields[0] {
                    "good" => "good",
                    "moderate" => "moderate",
                    "poor" => "poor",
                    _ => anyhow::bail!("invalid preset"),
                },
                sample_rate: fields[1].parse()?,
                seed: u64::from_str_radix(fields[2].trim_start_matches("0x"), 16)?,
                delay_duration_s: fields[3].parse()?,
                doppler_duration_s: fields[4].parse()?,
                delay_set_ms: parse(5)?,
                delay_measured_ms: parse(6)?,
                path_power_ratio_db: parse(7)?,
                spread_set_hz: parse(8)?,
                spread_measured_hz: parse(9)?,
                noise_ref_error_db: parse(10)?,
                within_tolerance: fields[11].parse()?,
            })
        })
        .collect()
}

pub fn table(rows: &[Measurement]) -> String {
    let mut out = String::from("| Preset | Delay set / measured | Path power ratio | Doppler spread (2 sigma) set / measured | 3 kHz noise reference error | Within tolerance |\n|---|---|---|---|---|---|\n");
    for r in rows {
        let label = match r.preset {
            "good" => "Good (MPG)",
            "poor" => "Poor (MPP)",
            _ => "Moderate",
        };
        out.push_str(&format!(
            "| {label} | {:.3} / {:.3} ms | {:+.2} dB | {:.2} / {:.2} Hz | {:+.2} dB | {} |\n",
            r.delay_set_ms,
            r.delay_measured_ms,
            r.path_power_ratio_db,
            r.spread_set_hz,
            r.spread_measured_hz,
            r.noise_ref_error_db,
            if r.within_tolerance { "yes" } else { "no" }
        ));
    }
    out
}

pub fn run(sample_rate: u32, out_dir: &Path) -> anyhow::Result<String> {
    anyhow::ensure!(
        (8000..=192000).contains(&sample_rate) && sample_rate % 100 == 0,
        "sample rate must be 8000..192000 and divisible by 100"
    );
    let rows = [
        measure(0, sample_rate, 600, 1200),
        measure(1, sample_rate, 600, 300),
        measure(2, sample_rate, 600, 300),
    ];
    fs::create_dir_all(out_dir)?;
    let mut csv = format!("{HEADER}\n");
    for row in &rows {
        csv.push_str(&row.to_csv());
        csv.push('\n');
    }
    fs::write(out_dir.join("channel-check.csv"), csv)?;
    Ok(table(&rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn two_tap_delay_estimate_is_exact() {
        let cfg = WattersonPreset::Poor.config();
        let (delay, _) = tap_measurement(&cfg, 8000, 42, 10);
        assert!((delay - 2.0).abs() < 0.01, "{delay}");
    }
    #[test]
    fn doppler_estimator_recovers_a_known_spread() {
        // A seeded AR(1) process with the Gaussian model's exact correlation
        // at the chosen lag exercises the estimator independent of the modem.
        use rand::{RngExt, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(41);
        let rate = 100.0;
        let lag = 75;
        let target = 1.0;
        let tau = lag as f64 / rate;
        let r = (-2.0 * PI * PI * (target / 2.0_f64).powi(2) * tau * tau).exp();
        let alpha = r.powf(1.0 / lag as f64);
        let beta = (1.0 - alpha * alpha).sqrt();
        let mut x = 0.0;
        let mut samples = Vec::new();
        for _ in 0..100_000 {
            x = alpha * x + beta * (rng.random::<f64>() - 0.5) * 12.0_f64.sqrt();
            samples.push((x, 0.0));
        }
        let measured = estimate_spread(&samples, lag, rate);
        assert!((measured - target).abs() < 0.15, "{measured}");
    }
    #[test]
    fn csv_round_trips() {
        let row = measure(2, 8000, 10, 15);
        let dir = std::env::temp_dir().join(format!("coppa-channel-check-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("channel-check.csv"),
            format!("{HEADER}\n{}\n", row.to_csv()),
        )
        .unwrap();
        let parsed = read_csv(&dir.join("channel-check.csv")).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].preset, row.preset);
        assert_eq!(parsed[0].sample_rate, row.sample_rate);
        fs::remove_dir_all(dir).unwrap();
    }
}
