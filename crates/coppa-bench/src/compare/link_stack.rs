//! COP-170: the in-process stack. Two library [`Link`]s run in virtual time,
//! joined by one [`PersistentChannel`] per direction, in 960-sample blocks
//! with one block of pipeline delay. The caller `N0AAA` dials at t = 0, the
//! answerer `N0BBB` listens, and the payload is written at the caller's
//! CONNECTED (what a TNC host does).

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use coppa_channel::persistent::{derive_seed, PersistentChannel, SnrProfile};
use coppa_engine::{CoppaCore, EngineConfig, Link, LinkConfig, LinkEvent};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use super::grid::Channel;
use super::trial::{TrialClock, TrialEvents, TrialRow};

/// Sample rate of every profile.
pub const SAMPLE_RATE: usize = 48_000;
/// Block size: 20 ms.
pub const BLOCK: usize = 960;
/// Seed stream for the payload bytes; the channel uses streams 0 and 1.
pub const STREAM_PAYLOAD: u8 = 2;
/// Payload of the frame the SNR reference is measured on.
const REFERENCE_FRAME_BYTES: usize = 40;

/// A measurable row of the comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSpec {
    /// Row name used on the CLI and in the CSV.
    pub name: &'static str,
    /// Stack name.
    pub stack: &'static str,
    /// Speed level both ends start at.
    pub level: u8,
    /// Whether the negotiated robust floor is on.
    pub robust_floor: bool,
    /// Mode label for tables.
    pub mode: &'static str,
}

/// Rows the runner knows. `link-default` is `LinkConfig::new`'s defaults.
pub const ROWS: [RowSpec; 3] = [
    RowSpec {
        name: "link-l2",
        stack: "link",
        level: 2,
        robust_floor: false,
        mode: "library Link, level 2 (BPSK 1/2), floor off",
    },
    RowSpec {
        name: "link-l4",
        stack: "link",
        level: 4,
        robust_floor: false,
        mode: "library Link, level 4, floor off",
    },
    RowSpec {
        name: "link-default",
        stack: "link",
        level: 1,
        robust_floor: true,
        mode: "library Link defaults (level 1, floor on)",
    },
];

/// Look up a row by name.
#[must_use]
pub fn row_spec(name: &str) -> Option<RowSpec> {
    ROWS.into_iter().find(|r| r.name == name)
}

/// One trial to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrialSpec {
    /// Channel.
    pub channel: Channel,
    /// SNR (dB, 3 kHz reference).
    pub snr_db: i32,
    /// Payload size; any size is allowed here, the grid uses 2048 and 16384.
    pub size_bytes: usize,
    /// Trial index within the cell.
    pub trial: usize,
    /// Seed.
    pub seed: u64,
}

/// Test hooks. Hidden: the published plan never sets them.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrialOptions {
    /// Feed silence to both receivers for the whole trial.
    pub muted: bool,
    /// Feed silence to both receivers from the caller's CONNECTED on.
    pub mute_after_connect: bool,
}

fn engine_config(level: u8) -> EngineConfig {
    EngineConfig {
        speed_level: level,
        ..EngineConfig::default()
    }
}

/// The SNR reference: clean mean power of one 40-byte data frame at `level`.
pub fn reference_power(level: u8) -> Result<f32> {
    let core = CoppaCore::with_config(engine_config(level));
    let payload: Vec<u8> = (0..REFERENCE_FRAME_BYTES as u8).collect();
    let audio = core
        .encode_bytes(&payload)
        .map_err(|e| anyhow!("reference frame at level {level}: {e}"))?;
    if audio.is_empty() {
        return Err(anyhow!("reference frame at level {level} is empty"));
    }
    Ok(audio.iter().map(|x| x * x).sum::<f32>() / audio.len() as f32)
}

/// The seeded payload of a trial.
#[must_use]
pub fn payload(seed: u64, size_bytes: usize) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(derive_seed(seed, 0, STREAM_PAYLOAD));
    (0..size_bytes).map(|_| rng.random::<u8>()).collect()
}

// The trial needs one real epoch; every later time is derived from the
// sample clock, so the result does not depend on wall time.
#[allow(clippy::disallowed_methods)]
fn epoch() -> Instant {
    Instant::now()
}

fn link_config(row: &RowSpec, call: &str) -> LinkConfig {
    let mut cfg = LinkConfig::new(call);
    cfg.robust_floor = row.robust_floor;
    cfg.engine = engine_config(row.level);
    cfg
}

/// Run one trial of `row`. Deterministic in `(row, spec, ref_power, options)`.
pub fn run_link_trial(
    row: &RowSpec,
    spec: &TrialSpec,
    ref_power: f32,
    options: TrialOptions,
) -> Result<TrialRow> {
    let clock = TrialClock::new(spec.size_bytes);
    let mut caller = Link::new(link_config(row, "N0AAA")).map_err(|e| anyhow!("{e}"))?;
    let mut answerer = Link::new(link_config(row, "N0BBB")).map_err(|e| anyhow!("{e}"))?;
    answerer.set_listening(true);

    let fading = spec.channel.preset().map(|p| p.config());
    let snr = Some(SnrProfile::constant(spec.snr_db as f32));
    let mut a_to_b = PersistentChannel::new(
        SAMPLE_RATE as f32,
        fading.as_ref(),
        snr.clone(),
        ref_power,
        spec.seed,
        0,
    );
    let mut b_to_a = PersistentChannel::new(
        SAMPLE_RATE as f32,
        fading.as_ref(),
        snr,
        ref_power,
        spec.seed,
        1,
    );
    let data = payload(spec.seed, spec.size_bytes);

    let base = epoch();
    caller
        .connect("N0BBB", base)
        .map_err(|e| anyhow!("dial: {e}"))?;

    let mut a_out = vec![0.0f32; BLOCK];
    let mut b_out = vec![0.0f32; BLOCK];
    let mut a_in = vec![0.0f32; BLOCK];
    let mut b_in = vec![0.0f32; BLOCK];
    let mut delivered: Vec<u8> = Vec::with_capacity(spec.size_bytes);
    let mut buf = vec![0u8; 4096];
    let mut ev = TrialEvents::default();
    let mut caller_gave_up = false;
    let mut block: u64 = 0;

    loop {
        let t = block as f64 * BLOCK as f64 / SAMPLE_RATE as f64;
        let now = base + Duration::from_millis(block * 20);
        caller.process(now, &a_in, &mut a_out);
        answerer.process(now, &b_in, &mut b_out);

        let mut ended = false;
        while let Some(event) = caller.poll_event() {
            match event {
                LinkEvent::Connected { .. } if ev.connected_at.is_none() => {
                    ev.connected_at = Some(t);
                    caller
                        .send(&data)
                        .map_err(|e| anyhow!("write payload: {e}"))?;
                }
                LinkEvent::Disconnected { .. } => {
                    if ev.connected_at.is_some() {
                        ended = true;
                    } else {
                        caller_gave_up = true;
                    }
                }
                _ => {}
            }
        }
        while let Some(event) = answerer.poll_event() {
            if matches!(event, LinkEvent::Disconnected { .. }) && ev.connected_at.is_some() {
                ended = true;
            }
        }
        loop {
            let n = answerer.recv(&mut buf);
            if n == 0 {
                break;
            }
            delivered.extend_from_slice(&buf[..n]);
        }

        block += 1;
        let t_end = block as f64 * BLOCK as f64 / SAMPLE_RATE as f64;
        if delivered.len() >= spec.size_bytes && ev.connected_at.is_some() {
            ev.completed_at = Some(t);
            ev.stopped_at = t;
            break;
        }
        if ended {
            ev.dropped_at = Some(t);
            ev.stopped_at = t;
            break;
        }
        match ev.connected_at {
            None if caller_gave_up || t_end >= super::trial::CONNECT_DEADLINE_S => {
                ev.stopped_at = t_end;
                break;
            }
            Some(c) if t_end - c >= clock.cap_s => {
                ev.stopped_at = t_end;
                break;
            }
            _ => {}
        }

        let mute = options.muted || (options.mute_after_connect && ev.connected_at.is_some());
        let next_b = a_to_b.process(&a_out);
        let next_a = b_to_a.process(&b_out);
        if mute {
            a_in.fill(0.0);
            b_in.fill(0.0);
        } else {
            a_in.copy_from_slice(&next_a);
            b_in.copy_from_slice(&next_b);
        }
    }

    ev.delivered = delivered.len();
    let c = clock.classify(&ev);
    let exact = delivered.len() <= data.len() && delivered[..] == data[..delivered.len()];
    Ok(TrialRow {
        row: row.name.to_string(),
        stack: row.stack.to_string(),
        level: row.level,
        robust_floor: row.robust_floor,
        channel: spec.channel,
        snr_db: spec.snr_db,
        size_bytes: spec.size_bytes,
        trial: spec.trial,
        seed: spec.seed,
        ref_power: f64::from(ref_power),
        cap_s: clock.cap_s,
        outcome: c.outcome,
        connect_s: c.connect_s,
        delivered_bytes: delivered.len().min(spec.size_bytes),
        exact,
        transfer_s: c.transfer_s,
        net_bytes_per_min: c.net_bytes_per_min,
        drops: c.drops,
        virtual_s: ev.stopped_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::trial::Outcome;

    #[test]
    fn reference_power_is_the_clean_frame_power() {
        // The planning probe measured 0.02182 at level 2 on a newer main with
        // its own payload bytes; 2% is 0.09 dB of SNR reference.
        let p = reference_power(2).unwrap();
        assert!((p / 0.021_82 - 1.0).abs() < 0.02, "L2 reference power {p}");
        assert!(reference_power(4).unwrap() < p);
        assert!(row_spec("link-l2").is_some());
        assert!(row_spec("vara").is_none());
    }

    #[test]
    fn no_connect_trial_is_deterministic() {
        let row = row_spec("link-l2").unwrap();
        let spec = TrialSpec {
            channel: Channel::Wgn,
            snr_db: 0,
            size_bytes: 2048,
            trial: 0,
            seed: 0x170c_0000,
        };
        let p = reference_power(row.level).unwrap();
        let first = run_link_trial(&row, &spec, p, TrialOptions::default()).unwrap();
        let second = run_link_trial(&row, &spec, p, TrialOptions::default()).unwrap();
        assert_eq!(first.to_line(), second.to_line());
        assert_eq!(first.outcome, Outcome::NoConnect, "{}", first.to_line());
    }
}
