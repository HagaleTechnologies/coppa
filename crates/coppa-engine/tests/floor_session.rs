//! Engine-side floor session coverage of the COP-190/COP-196 connect bar:
//! the engine constructs a separately detected floor receive path next to
//! its OFDM paths, floor audio decodes through the seeded persistent
//! channel with zero wrong payloads, the v1 OFDM path is unaffected, and
//! paired library Links are driven through the production audio paths at
//! -6 dB AWGN under the COP-190 fixed trial contract. The negotiated,
//! floor-aware session controller (fallback, OFDM probe, floor-carried
//! handshake legs) is COP-195's work: the connection trials below are
//! authored here against COP-190 and report their measured outcomes, so
//! the missing controller shows up as failing trials rather than as
//! absent coverage; see `docs/LIMITATIONS.md`, COP-169 section.

use std::time::{Duration, Instant};

use coppa_channel::persistent::{PersistentChannel, SnrProfile};
use coppa_engine::{CoppaCore, Link, LinkConfig, LinkState};

const SAMPLE_RATE: usize = 48_000;

/// COP-190 connect-bar contract: ten trials, seeds `0x16910000 + index`,
/// 48 kHz, constant -6 dB AWGN, 960-sample blocks on one sample clock,
/// both peers Connected by sample 2_880_000 (60 simulated seconds).
const TRIALS: usize = 10;
const SEED_BASE: u64 = 0x1691_0000;
const SNR_DB: f32 = -6.0;
const BLOCK: usize = 960;
const DEADLINE_SAMPLES: usize = 2_880_000;

/// Clean active reference power of the production floor waveform at its
/// transmitted amplitude, measured once per run (never from silence and
/// never from the noisy channel output) and reused by every trial, per
/// the persistent-channel measurement contract.
fn floor_reference_power() -> f32 {
    let core = CoppaCore::new();
    let clean = core.floor().encode(b"connect bar reference").unwrap();
    clean.iter().map(|x| x * x).sum::<f32>() / clean.len() as f32
}

/// One recorded trial outcome, in the COP-190 evidence CSV shape.
#[derive(Debug)]
struct TrialOutcome {
    index: usize,
    seed: u64,
    connected: bool,
    elapsed_s: f32,
    a_state: String,
    b_state: String,
    failure_reason: String,
}

impl TrialOutcome {
    /// The CSV row for `results/cop-190/connect/link.csv`. Failure reasons
    /// are kept comma-free so the row stays trivially parseable; an empty
    /// reason (success rows) is written as `-`.
    fn csv_row(&self) -> String {
        format!(
            "{},{:#x},{},{},{:.1},{},{},{}",
            self.index,
            self.seed,
            SNR_DB,
            self.connected,
            self.elapsed_s,
            self.a_state,
            self.b_state,
            if self.failure_reason.is_empty() {
                "-".to_string()
            } else {
                self.failure_reason.clone()
            }
        )
    }
}

/// Run one paired-Link trial under the fixed contract: fresh Links and
/// fresh persistent channels per trial, persistent state throughout the
/// trial, both directions processed every block (noisy silence and
/// turnaround included), one block of pipeline delay so neither peer gets
/// an instantaneous response, half-duplex reception and actual
/// transmission lengths respected, `Instant` supplied from sample time.
///
/// `muted` suppresses all peer audio (each receiver hears silence): the
/// negative-control shape that proves this runner cannot report
/// connection success from local intent alone. `deadline_samples` bounds
/// the simulated window.
#[allow(clippy::disallowed_methods)]
// The trial driver is Link's caller, so it owns the clock: this is the
// one legitimate `Instant::now` (COP-139's own test-side precedent in
// `link/tests.rs`), fixed once as the sample-time origin.
fn run_link_trial(
    index: usize,
    reference_power: f32,
    muted: bool,
    deadline_samples: usize,
) -> TrialOutcome {
    let seed = SEED_BASE + index as u64;
    let mut a = Link::new(LinkConfig::new("VK3ABC")).unwrap();
    let mut b = Link::new(LinkConfig::new("W1AW")).unwrap();
    a.set_listening(true);
    b.set_listening(true);
    // Direction 0 is A to B; direction 1 is B to A (independent seeds per
    // the persistent-channel contract).
    let mut a_to_b = PersistentChannel::new(
        SAMPLE_RATE as f32,
        None,
        Some(SnrProfile::constant(SNR_DB)),
        reference_power,
        seed,
        0,
    );
    let mut b_to_a = PersistentChannel::new(
        SAMPLE_RATE as f32,
        None,
        Some(SnrProfile::constant(SNR_DB)),
        reference_power,
        seed,
        1,
    );
    let start = Instant::now();
    a.connect("W1AW", start).expect("idle caller");
    let mut carried_to_a: Vec<f32> = Vec::new();
    let mut carried_to_b: Vec<f32> = Vec::new();
    let mut tx_a = vec![0.0f32; BLOCK];
    let mut tx_b = vec![0.0f32; BLOCK];
    let mut connected_at: Option<usize> = None;
    let mut samples = 0usize;
    while samples < deadline_samples {
        let now = start + Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64);
        // Each peer hears the previous tick's channel output, so call
        // ordering gives neither side an instantaneous response.
        a.process(now, &carried_to_a, &mut tx_a);
        while a.poll_event().is_some() {}
        b.process(now, &carried_to_b, &mut tx_b);
        while b.poll_event().is_some() {}
        samples += BLOCK;
        let heard_by_b = a_to_b.process(&tx_a);
        let heard_by_a = b_to_a.process(&tx_b);
        carried_to_b = if muted {
            vec![0.0; heard_by_b.len()]
        } else {
            heard_by_b
        };
        carried_to_a = if muted {
            vec![0.0; heard_by_a.len()]
        } else {
            heard_by_a
        };
        // Both peers confirmed, matching identities, and decoded session
        // traffic at both ends: one endpoint's early Connected event alone
        // does not qualify.
        if a.state() == LinkState::Connected
            && b.state() == LinkState::Connected
            && a.remote() == Some("W1AW")
            && b.remote() == Some("VK3ABC")
            && a.frames_received() > 0
            && b.frames_received() > 0
        {
            connected_at = Some(samples);
            break;
        }
    }
    let a_state = a.state().to_string();
    let b_state = b.state().to_string();
    let (connected, elapsed_s, failure_reason) = match connected_at {
        Some(at) => (true, at as f32 / SAMPLE_RATE as f32, String::new()),
        None => (
            false,
            deadline_samples as f32 / SAMPLE_RATE as f32,
            format!(
                "timeout: A={} B={} decoded=({} / {})",
                a_state,
                b_state,
                a.frames_received(),
                b.frames_received()
            )
            .replace(',', ";"),
        ),
    };
    TrialOutcome {
        index,
        seed,
        connected,
        elapsed_s,
        a_state,
        b_state,
        failure_reason,
    }
}

/// Run all ten trials (never a subset), print every outcome including
/// failures, and export the evidence CSV when `COPPA_FLOOR_CONNECT_OUT`
/// names an output directory. Default test execution writes nothing.
fn run_link_trials() -> Vec<TrialOutcome> {
    let reference_power = floor_reference_power();
    println!("reference power {reference_power:.6} (clean floor mean square)");
    let mut outcomes = Vec::new();
    for index in 0..TRIALS {
        let outcome = run_link_trial(index, reference_power, false, DEADLINE_SAMPLES);
        println!(
            "link trial {} seed {:#x}: connected={} elapsed={:.1}s A={} B={} reason={}",
            outcome.index,
            outcome.seed,
            outcome.connected,
            outcome.elapsed_s,
            outcome.a_state,
            outcome.b_state,
            outcome.failure_reason
        );
        outcomes.push(outcome);
    }
    if let Ok(dir) = std::env::var("COPPA_FLOOR_CONNECT_OUT") {
        let path = std::path::Path::new(&dir).join("link.csv");
        let mut csv =
            String::from("trial,seed,snr_db,connected,elapsed_s,a_state,b_state,failure_reason\n");
        for row in &outcomes {
            csv.push_str(&row.csv_row());
            csv.push('\n');
        }
        std::fs::write(&path, csv).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        println!("wrote {}", path.display());
    }
    outcomes
}

/// COP-196 AC1: two listening/calling library Links, using normal
/// floor-enabled discovery and the production receive/transmit paths, must
/// both reach Connected within 60 simulated seconds at -6 dB AWGN in at
/// least nine of the ten fixed-seed trials.
///
/// Ignored because a full ten-trial release run is expensive; run
/// explicitly per `results/cop-190/connect/metadata.md`. While COP-195's
/// floor session controller is not incorporated, the trials are expected
/// to fail the nine-of-ten bar; that measured outcome is the recorded
/// evidence for the missing controller, not a reason to change seeds,
/// thresholds, gains or deadlines.
#[test]
#[ignore = "expensive release measurement; see results/cop-190/connect/metadata.md"]
fn floor_link_connect_minus6() {
    let outcomes = run_link_trials();
    let connected = outcomes.iter().filter(|o| o.connected).count();
    println!(
        "link connected {connected}/{} at {SNR_DB} dB",
        outcomes.len()
    );
    assert!(
        connected >= 9,
        "connect bar not met: {connected}/{} trials connected; failures: {}",
        outcomes.len(),
        outcomes
            .iter()
            .filter(|o| !o.connected)
            .map(|o| format!("seed {:#x}: {}", o.seed, o.failure_reason))
            .collect::<Vec<_>>()
            .join(" | ")
    );
}

/// Negative control for the trial runner: with all peer audio suppressed
/// the same runner must not report connection success, so a passing bar
/// can never come from local intent (this end keying up and counting its
/// own handshake attempts) rather than decoded peer traffic.
#[test]
fn floor_link_muted_peers_never_connect() {
    let reference_power = floor_reference_power();
    let outcome = run_link_trial(0, reference_power, true, 5 * SAMPLE_RATE);
    assert!(
        !outcome.connected,
        "muted peers cannot establish a session: {outcome:?}"
    );
    assert_eq!(
        (outcome.a_state.as_str(), outcome.b_state.as_str()),
        ("CONNECTING", "IDLE"),
        "with no peer audio the caller is still dialing and the listener idle"
    );
}

/// COP-196 AC3: audit the committed connection evidence. Checks both CSVs
/// (exact header, exact seed set 0x16910000..0x16910009, one row per trial
/// index, -6 dB rows only), finite in-range elapsed times (success rows
/// record when both ends first qualified, timeout rows record 60.0),
/// consistency between `connected` and `failure_reason`, and separate pass
/// counts of at least nine per adapter.
#[test]
fn floor_connection_recordings_are_complete() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for adapter in ["link", "daemon"] {
        let path = root
            .join("results/cop-190/connect")
            .join(format!("{adapter}.csv"));
        let csv = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let connected = audit_connect_csv(&path.display().to_string(), &csv)
            .unwrap_or_else(|e| panic!("{adapter} evidence rejected: {e}"));
        assert!(
            connected >= 9,
            "{adapter} connect bar not met: {connected}/{TRIALS} recorded trials connected"
        );
    }
}

/// The audit core, shared with the malformed-record case: returns the
/// adapter's pass count or rejects the recording with a reason.
fn audit_connect_csv(name: &str, csv: &str) -> Result<usize, String> {
    let mut lines = csv.lines();
    let header = lines.next().ok_or_else(|| format!("{name}: empty file"))?;
    if header != "trial,seed,snr_db,connected,elapsed_s,a_state,b_state,failure_reason" {
        return Err(format!("{name}: wrong header {header:?}"));
    }
    let mut seen = [false; TRIALS];
    let mut connected = 0usize;
    let mut rows = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        rows += 1;
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() != 8 {
            return Err(format!(
                "{name}: row {rows} has {} fields, not 8",
                fields.len()
            ));
        }
        let index: usize = fields[0]
            .parse()
            .map_err(|e| format!("{name}: row {rows} bad trial index: {e}"))?;
        if index >= TRIALS {
            return Err(format!(
                "{name}: row {rows} trial index {index} out of range"
            ));
        }
        if seen[index] {
            return Err(format!("{name}: duplicate trial {index}"));
        }
        seen[index] = true;
        let seed = u64::from_str_radix(fields[1].trim_start_matches("0x"), 16)
            .map_err(|e| format!("{name}: trial {index} bad seed: {e}"))?;
        if seed != SEED_BASE + index as u64 {
            return Err(format!(
                "{name}: trial {index} seed {seed:#x} is not the fixed {:#x}",
                SEED_BASE + index as u64
            ));
        }
        if fields[2] != "-6" {
            return Err(format!("{name}: trial {index} snr {} is not -6", fields[2]));
        }
        let ok = match fields[3] {
            "true" => true,
            "false" => false,
            other => {
                return Err(format!(
                    "{name}: trial {index} bad connected flag {other:?}"
                ))
            }
        };
        let elapsed: f32 = fields[4]
            .parse()
            .map_err(|e| format!("{name}: trial {index} bad elapsed_s: {e}"))?;
        if !elapsed.is_finite() || elapsed <= 0.0 || elapsed > 60.0 {
            return Err(format!(
                "{name}: trial {index} elapsed {elapsed}s out of range (0, 60]"
            ));
        }
        let reason = fields[7];
        if ok {
            connected += 1;
            if reason != "-" {
                return Err(format!(
                    "{name}: trial {index} connected row carries failure reason {reason:?}"
                ));
            }
        } else if reason == "-" {
            return Err(format!(
                "{name}: trial {index} failed row has no failure reason"
            ));
        } else if (elapsed - 60.0).abs() > 0.05 {
            return Err(format!(
                "{name}: trial {index} timeout recorded {elapsed}s, not the 60s deadline"
            ));
        }
    }
    for (index, was_seen) in seen.iter().enumerate() {
        if !was_seen {
            return Err(format!(
                "{name}: no row for trial {index} (seed {:#x})",
                SEED_BASE + index as u64
            ));
        }
    }
    if rows != TRIALS {
        return Err(format!("{name}: {rows} rows, not {TRIALS}"));
    }
    Ok(connected)
}

/// Malformed-record case: the audit must reject an incomplete seed set
/// (nine rows, one seed missing) rather than blessing partial evidence.
#[test]
fn floor_connection_audit_rejects_an_incomplete_seed_set() {
    let mut csv =
        String::from("trial,seed,snr_db,connected,elapsed_s,a_state,b_state,failure_reason\n");
    for index in 0..TRIALS {
        if index == 7 {
            continue;
        }
        csv.push_str(&format!(
            "{},{:#x},-6,false,60.0,IDLE,IDLE,timeout\n",
            index,
            SEED_BASE + index as u64
        ));
    }
    let rejected = audit_connect_csv("malformed.csv", &csv)
        .expect_err("an incomplete seed set must be rejected");
    assert!(
        rejected.contains("seed 0x16910007") || rejected.contains("trial 7"),
        "rejection must name the missing trial: {rejected}"
    );
}

#[test]
fn engine_constructs_a_separately_detected_floor_receive_path() {
    let mut core = CoppaCore::new();
    let payload = b"engine floor path";
    let audio = core.floor().encode(payload).unwrap();
    assert_eq!(audio.len(), 8 * SAMPLE_RATE);
    assert_eq!(core.floor().receive(&audio).unwrap(), payload.to_vec());
    // The continuous detector delivers the same frame exactly once, from
    // arbitrary chunk shapes.
    let mut stream_audio = vec![0.0f32; SAMPLE_RATE];
    stream_audio.extend(&audio);
    stream_audio.extend(vec![0.0; 2 * SAMPLE_RATE]);
    let mut delivered = Vec::new();
    for chunk in stream_audio.chunks(317) {
        delivered.extend(core.push_floor_samples(chunk));
    }
    assert_eq!(delivered, [payload.to_vec()]);
    // The OFDM transceiver still decodes its own v1 frames (regression
    // guard for the shared-engine case).
    let ofdm_audio = core.encode("v1").expect("v1 transmit");
    assert_eq!(core.decode(&ofdm_audio).expect("v1 decode"), "v1");
}

#[test]
fn floor_audio_through_the_persistent_channel_decodes_exactly() {
    let mut core = CoppaCore::new();
    let payload = b"persistent engine floor";
    let clean = core.floor().encode(payload).unwrap();
    let active_power: f32 = clean.iter().map(|x| x * x).sum::<f32>() / clean.len() as f32;
    let mut channel = PersistentChannel::new(
        SAMPLE_RATE as f32,
        None,
        Some(SnrProfile::constant(-6.0)),
        active_power,
        0x1691_0000,
        1,
    );
    let mut received = Vec::new();
    let mut position = 0;
    while position < clean.len() {
        let end = (position + 9600).min(clean.len());
        let block = channel.process(&clean[position..end]);
        received.extend(core.push_floor_samples(&block));
        position = end;
    }
    received.extend(core.push_floor_samples(&vec![0.0; 4 * SAMPLE_RATE]));
    assert_eq!(received, [payload.to_vec()]);
}

// The paired connection trials above cover the COP-190 connect bar for
// library Links (and, in coppa-daemon, for EventLoop pairs) against the
// built physical layer. The negotiated floor session controller that must
// carry the handshake legs at -6 dB -- floor-aware discovery, fallback and
// OFDM probing -- remains COP-195's scope: until it is incorporated these
// trials measure and record its absence as failing trials.
