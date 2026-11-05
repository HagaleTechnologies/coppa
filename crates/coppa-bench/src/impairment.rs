//! Versioned synthetic transfer presets and independent-frame measurements.
use crate::{
    scenario::{ChannelSpec, SAMPLE_RATE},
    transfer::{apply_transfer_channel_with_impairment, recovery_fraction, TransferPhy, V1Phy},
};
use clap::{Parser, ValueEnum};
use rand::{rngs::StdRng, RngExt, SeedableRng};
use std::fmt::Write as _;

/// Fixed presets. Parameter changes require new versioned labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TransferImpairment {
    None,
    Impulsive,
    Clipping,
    Agc,
    Tilt,
}
impl TransferImpairment {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Impulsive,
        Self::Clipping,
        Self::Agc,
        Self::Tilt,
    ];
    /// Report identity and complete preset settings.
    pub fn metadata(self) -> (&'static str, &'static str) {
        match self {
            Self::None => ("none-v1","none"),
            Self::Impulsive => ("impulsive-v1","p=0.001;amplitude_rms=10"),
            Self::Clipping => ("clipping-v1","threshold_rms=2"),
            Self::Agc => ("agc-v1","fade_hz=1;fade_db=20;target_rms=0.5;block_samples=480;beta_attack=0.6;beta_release=0.85;gain_min=0.0001;gain_max=10000"),
            Self::Tilt => ("tilt-v1","low_hz=300;high_hz=2700;low_db=-6;high_db=6"),
        }
    }
}
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Selection {
    All,
    None,
    Impulsive,
    Clipping,
    Agc,
    Tilt,
}
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Channel {
    Awgn,
    Good,
    Moderate,
    Poor,
}
fn positive(s: &str) -> Result<usize, String> {
    s.parse::<usize>()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| "must be a positive integer".into())
}
fn finite(s: &str) -> Result<f32, String> {
    s.parse::<f32>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "SNR must be finite".into())
}
/// Validated diagnostic arguments. Parsing rejects bad domains before encoding.
#[derive(Debug, Parser)]
pub struct Options {
    #[arg(long, default_value_t=2, value_parser=clap::value_parser!(u8).range(1..=9))]
    level: u8,
    #[arg(long, default_value_t=2, value_parser=positive)]
    frames: usize,
    #[arg(long, default_value_t=10, value_parser=positive)]
    trials: usize,
    #[arg(long, default_value_t=30.0, value_parser=finite, allow_hyphen_values=true)]
    snr: f32,
    #[arg(long, default_value_t = 12648430)]
    seed: u64,
    #[arg(long, value_enum, default_value = "awgn")]
    channel: Channel,
    #[arg(long, value_enum, default_value = "all")]
    impairment: Selection,
}
impl Options {
    fn cases(&self) -> Vec<TransferImpairment> {
        match self.impairment {
            Selection::All => TransferImpairment::ALL.to_vec(),
            Selection::None => vec![TransferImpairment::None],
            Selection::Impulsive => vec![TransferImpairment::Impulsive],
            Selection::Clipping => vec![TransferImpairment::Clipping],
            Selection::Agc => vec![TransferImpairment::Agc],
            Selection::Tilt => vec![TransferImpairment::Tilt],
        }
    }
    fn channel(&self) -> (ChannelSpec, &'static str) {
        use coppa_channel::watterson::WattersonPreset::*;
        match self.channel {
            Channel::Awgn => (ChannelSpec::Awgn, "awgn"),
            Channel::Good => (ChannelSpec::Watterson(Good), "watterson-good"),
            Channel::Moderate => (ChannelSpec::Watterson(Moderate), "watterson-moderate"),
            Channel::Poor => (ChannelSpec::Watterson(Poor), "watterson-poor"),
        }
    }
}

// Each receiver is fresh even after a decode error. Legacy V1 metrics are unchanged.
struct IndependentV1 {
    level: u8,
    tx: V1Phy,
}
impl IndependentV1 {
    fn new(level: u8, frames: usize) -> Self {
        Self {
            level,
            tx: V1Phy::new(level, frames),
        }
    }
}
impl TransferPhy for IndependentV1 {
    fn frames_per_transfer(&self) -> usize {
        self.tx.frames_per_transfer()
    }
    fn payload_bytes(&self) -> usize {
        self.tx.payload_bytes()
    }
    fn encode_transfer(&self, payload: &[u8]) -> Vec<Vec<f32>> {
        self.tx.encode_transfer(payload)
    }
    fn decode_transfer(&self, windows: &[&[f32]]) -> Vec<u8> {
        windows
            .iter()
            .flat_map(|w| V1Phy::new(self.level, 1).decode_transfer(&[w]))
            .collect()
    }
}
/// A measurement with loss-weighted goodput and encoded audio airtime.
#[derive(Debug)]
pub struct Measurement {
    impairment: TransferImpairment,
    recovery: f64,
    goodput_bps: f64,
    airtime_s: f64,
}
impl Measurement {
    fn new(impairment: TransferImpairment, bytes: usize, samples: usize, recovery: f64) -> Self {
        let airtime_s = samples as f64 / SAMPLE_RATE as f64;
        Self {
            impairment,
            recovery,
            goodput_bps: bytes as f64 * 8.0 * recovery / airtime_s,
            airtime_s,
        }
    }
}
/// Run every selected case before publishing any CSV. Failed decodes count as
/// zero-filled recovered frames, including accidental byte matches.
pub fn run(options: &Options) -> Result<Vec<Measurement>, String> {
    let phy = IndependentV1::new(options.level, options.frames);
    let mut rows = Vec::new();
    for impairment in options.cases() {
        let mut recovery = 0.0;
        let mut samples = 0;
        for trial in 0..options.trials {
            let seed = options.seed.wrapping_add(trial as u64);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload: Vec<u8> = (0..phy.payload_bytes()).map(|_| rng.random()).collect();
            let frames = phy.encode_transfer(&payload);
            samples = frames.iter().map(Vec::len).sum();
            let received = apply_transfer_channel_with_impairment(
                &frames,
                options.channel().0,
                options.snr,
                seed,
                impairment,
            );
            if received.iter().flatten().any(|v| !v.is_finite()) {
                return Err("channel produced nonfinite samples; choose a less extreme SNR".into());
            }
            let windows: Vec<_> = received.iter().map(Vec::as_slice).collect();
            recovery += recovery_fraction(&payload, &phy.decode_transfer(&windows));
        }
        let row = Measurement::new(
            impairment,
            phy.payload_bytes(),
            samples,
            recovery / options.trials as f64,
        );
        if !row.goodput_bps.is_finite() || !row.airtime_s.is_finite() {
            return Err("nonfinite transfer metric".into());
        }
        rows.push(row);
    }
    Ok(rows)
}
/// Render a complete report, including rows for failed transfers.
pub fn csv(options: &Options, rows: &[Measurement]) -> String {
    let mut s=String::from("phy,level,channel,impairment,parameters,snr_db,frames,trials,seed,recovery_fraction,goodput_bps,airtime_s\n");
    for row in rows {
        let (label, parameters) = row.impairment.metadata();
        writeln!(
            s,
            "v1-independent,{},{},{},{},{:.1},{},{},{},{:.6},{:.2},{:.3}",
            options.level,
            options.channel().1,
            label,
            parameters,
            options.snr,
            options.frames,
            options.trials,
            options.seed,
            row.recovery,
            row.goodput_bps,
            row.airtime_s
        )
        .expect("writing to a String");
    }
    s
}

#[cfg(test)]
mod channel_tests {
    use super::*;
    use coppa_channel::{
        impairments::*,
        watterson::{watterson, WattersonPreset},
    };
    #[test]
    fn full_buffer_composition_and_disabled_compatibility() {
        let mut signal: Vec<f32> = (0..24000).map(|i| (i as f32 * 0.2).sin() * 0.3).collect();
        signal[12000] = 3.0;
        let frames = vec![signal[..7001].to_vec(), vec![], signal[7001..].to_vec()];
        let seed = 99;
        let sr = SAMPLE_RATE as f32;
        let power = coppa_channel::mean_power(&signal);
        let rms = power.sqrt();
        for channel in [
            ChannelSpec::Awgn,
            ChannelSpec::Watterson(WattersonPreset::Good),
            ChannelSpec::Watterson(WattersonPreset::Moderate),
            ChannelSpec::Watterson(WattersonPreset::Poor),
        ] {
            let baseline = crate::transfer::apply_transfer_channel(&frames, channel, 30.0, seed);
            for case in TransferImpairment::ALL {
                let shaped = match case {
                    TransferImpairment::Tilt => frequency_tilt(&signal, 12.0, sr).unwrap(),
                    TransferImpairment::Agc => coppa_channel::fading(&signal, 1.0, 20.0, sr),
                    _ => signal.clone(),
                };
                let faded = match channel {
                    ChannelSpec::Awgn => shaped,
                    ChannelSpec::Watterson(p) => {
                        watterson(&shaped, sr, &p.config(), seed ^ 0x3333_3333_3333_3333)
                    }
                };
                let noisy = coppa_channel::awgn_ref_seeded(
                    &faded,
                    30.0,
                    power,
                    sr,
                    seed ^ 0x5555_5555_5555_5555,
                );
                let expected = match case {
                    TransferImpairment::Impulsive => impulsive_noise_seeded(
                        &noisy,
                        0.001,
                        10.0 * rms,
                        seed ^ 0x7777_7777_7777_7777,
                    )
                    .unwrap(),
                    TransferImpairment::Clipping => hard_clip(&noisy, 2.0 * rms).unwrap(),
                    TransferImpairment::Agc => agc(&noisy, 0.5, 480).unwrap(),
                    _ => noisy,
                };
                let got =
                    apply_transfer_channel_with_impairment(&frames, channel, 30.0, seed, case);
                assert_eq!(
                    got.iter().map(Vec::len).collect::<Vec<_>>(),
                    vec![7001, 0, 16999]
                );
                assert_eq!(got.concat(), expected);
                if case == TransferImpairment::None {
                    assert_eq!(got, baseline);
                } else {
                    assert_ne!(got, baseline);
                }
            }
        }
    }
    #[test]
    fn empty_and_silent_transfers() {
        for case in TransferImpairment::ALL {
            assert!(
                apply_transfer_channel_with_impairment(&[], ChannelSpec::Awgn, 30.0, 1, case)
                    .is_empty()
            );
            assert_eq!(
                apply_transfer_channel_with_impairment(
                    &[vec![], vec![0.0; 10]],
                    ChannelSpec::Awgn,
                    30.0,
                    1,
                    case
                ),
                vec![vec![], vec![0.0; 10]]
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_domains_and_selectors() {
        let a = Options::try_parse_from(["bench"]).unwrap();
        assert_eq!(
            (a.level, a.frames, a.trials, a.snr, a.seed),
            (2, 2, 10, 30.0, 12648430)
        );
        assert_eq!(a.cases(), TransferImpairment::ALL);
        for (name, case) in [
            ("none", TransferImpairment::None),
            ("impulsive", TransferImpairment::Impulsive),
            ("clipping", TransferImpairment::Clipping),
            ("agc", TransferImpairment::Agc),
            ("tilt", TransferImpairment::Tilt),
        ] {
            assert_eq!(
                Options::try_parse_from(["b", "--impairment", name])
                    .unwrap()
                    .cases(),
                vec![case]
            );
        }
        for (flag, value) in [
            ("--level", "0"),
            ("--level", "10"),
            ("--frames", "0"),
            ("--trials", "0"),
            ("--snr", "NaN"),
            ("--snr", "inf"),
            ("--impairment", "bad"),
            ("--channel", "bad"),
        ] {
            assert!(
                Options::try_parse_from(["b", flag, value]).is_err(),
                "{flag} {value}"
            );
        }
        assert!(Options::try_parse_from(["b", "--snr", "-10"]).is_ok());
    }
    #[test]
    fn metrics_and_failed_rows() {
        let a = Options::try_parse_from(["b"]).unwrap();
        let row = Measurement::new(TransferImpairment::None, 234, 131040, 0.5);
        assert!((row.goodput_bps - 342.857142).abs() < 0.001);
        assert_eq!(row.airtime_s, 2.73);
        let csv = csv(
            &a,
            &[
                row,
                Measurement::new(TransferImpairment::Tilt, 234, 131040, 0.0),
            ],
        );
        assert!(csv.contains("0.500000,342.86,2.730"));
        assert!(csv.contains("0.000000,0.00,2.730"));
        assert!(csv.lines().all(|l| l.split(',').count() == 12));
    }
    #[test]
    fn independent_decoders_and_real_all_rows() {
        let a = Options::try_parse_from(["b", "--trials", "1"]).unwrap();
        let phy = IndependentV1::new(2, 2);
        let frames = phy.encode_transfer(&vec![42; phy.payload_bytes()]);
        let bad = vec![0.0; frames[0].len()];
        let expected = [
            V1Phy::new(2, 1).decode_transfer(&[&bad]),
            V1Phy::new(2, 1).decode_transfer(&[&frames[1]]),
        ]
        .concat();
        assert_eq!(phy.decode_transfer(&[&bad, &frames[1]]), expected);
        phy.decode_transfer(&[&bad, &bad]);
        assert_eq!(phy.decode_transfer(&[&bad, &frames[1]]), expected);
        let rows = run(&a).unwrap();
        assert_eq!(rows.len(), 5);
        for (row, case) in rows.iter().zip(TransferImpairment::ALL) {
            assert_eq!(row.impairment, case);
            assert!((0.0..=1.0).contains(&row.recovery));
            assert!(row.airtime_s > 0.0);
            assert!(
                (row.goodput_bps - phy.payload_bytes() as f64 * 8.0 * row.recovery / row.airtime_s)
                    .abs()
                    < 1e-9
            );
        }
    }
}
