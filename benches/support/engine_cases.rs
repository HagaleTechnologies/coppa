//! Common, verified workloads for engine timing and ideal frame measurements.

use coppa_engine::{ladder::speed_ladder, CoppaCore, EngineConfig};
use std::fmt::Write;

pub const PAYLOAD: &str = "0123456789ABCDEF0123456789ABCDEF";

#[derive(Debug, PartialEq)]
pub struct FrameMetrics {
    pub airtime_s: f64,
    pub goodput_bps: f64,
}

pub fn frame_metrics(
    sample_count: usize,
    sample_rate: u32,
    recovered_bytes: usize,
) -> Result<FrameMetrics, String> {
    if sample_count == 0 || sample_rate == 0 {
        return Err("frame sample count and sample rate must be nonzero".into());
    }
    let airtime_s = sample_count as f64 / f64::from(sample_rate);
    Ok(FrameMetrics {
        airtime_s,
        goodput_bps: recovered_bytes as f64 * 8.0 / airtime_s,
    })
}

pub struct EngineCase {
    pub level: u8,
    pub profile: &'static str,
    pub core: CoppaCore,
    pub samples: Vec<f32>,
    pub metrics: FrameMetrics,
}

pub fn prepare_cases() -> Result<Vec<EngineCase>, String> {
    speed_ladder()
        .into_iter()
        .map(|row| {
            let level = row.level;
            let core = CoppaCore::with_config(EngineConfig {
                speed_level: level,
                ..Default::default()
            });
            let samples = core
                .encode(PAYLOAD)
                .map_err(|error| format!("level {level}: setup encode failed: {error}"))?;
            let recovered = core
                .decode(&samples)
                .map_err(|error| format!("level {level}: setup decode failed: {error}"))?;
            if recovered != PAYLOAD {
                return Err(format!("level {level}: setup payload mismatch"));
            }
            let metrics = frame_metrics(samples.len(), core.config().sample_rate, recovered.len())
                .map_err(|error| format!("level {level}: {error}"))?;
            Ok(EngineCase {
                level,
                profile: if core.on_vhf_profile() {
                    "vhf_wide"
                } else {
                    "hf_standard"
                },
                core,
                samples,
                metrics,
            })
        })
        .collect()
}

pub fn frame_report(cases: &[EngineCase]) -> String {
    let mut report = format!(
        "Engine frame metrics: ideal clean loopback\n\
         Payload: {} bytes; compression: off; sample rate: {} Hz; CP: long\n\
         Goodput excludes channel losses, ARQ, turnaround and session overhead.\n\
         level,profile,payload_bytes,frame_samples,frame_airtime_s,ideal_frame_goodput_bps\n",
        PAYLOAD.len(),
        cases
            .first()
            .expect("verified engine cases")
            .core
            .config()
            .sample_rate,
    );
    for case in cases {
        writeln!(
            report,
            "{},{},{},{},{:.6},{:.2}",
            case.level,
            case.profile,
            PAYLOAD.len(),
            case.samples.len(),
            case.metrics.airtime_s,
            case.metrics.goodput_bps,
        )
        .expect("write to String");
    }
    report
}
