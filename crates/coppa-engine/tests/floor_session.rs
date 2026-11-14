//! Engine-side floor session coverage of the COP-190/COP-196 connect bar:
//! the engine constructs a separately detected floor receive path next to
//! its OFDM paths, floor audio decodes through the seeded persistent
//! channel with zero wrong payloads, the v1 OFDM path is unaffected, and
//! paired library Links are driven through the production audio paths at
//! -6 dB AWGN under the COP-190 fixed trial contract. The negotiated,
//! floor-aware session controller (`coppa_protocol::floor_controller`,
//! COP-195) carries fallback, the OFDM probe and floor-carried handshake
//! legs; the connection trials below measure it, and the audit checks the
//! recorded outcomes for both the library Link and the coppad adapter
//! (COP-202). Open gaps: `docs/LIMITATIONS.md`, COP-169 section.

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
/// explicitly per `results/cop-190/connect/metadata.md`. Link runs
/// COP-195's floor session controller, so the trials measure it. A missed
/// bar is a reason to investigate timing, not to change seeds, thresholds,
/// gains or deadlines.
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
    if let Err(e) = audit_connect_dir(&root.join("results/cop-190/connect")) {
        panic!("{e}");
    }
}

/// Every adapter the connect bar covers: the library Link and, since
/// COP-202, coppad's event loop.
const ADAPTERS: [&str; 2] = ["link", "daemon"];

/// Audit one recording set: each adapter's CSV must exist, pass
/// `audit_connect_csv` and record at least nine connected trials. The error
/// names the first adapter that fails.
fn audit_connect_dir(dir: &std::path::Path) -> Result<(), String> {
    for adapter in ADAPTERS {
        let path = dir.join(format!("{adapter}.csv"));
        let csv = std::fs::read_to_string(&path)
            .map_err(|e| format!("{adapter} evidence missing: read {}: {e}", path.display()))?;
        let connected = audit_connect_csv(&path.display().to_string(), &csv)
            .map_err(|e| format!("{adapter} evidence rejected: {e}"))?;
        if connected < 9 {
            return Err(format!(
                "{adapter} connect bar not met: {connected}/{TRIALS} recorded trials connected"
            ));
        }
    }
    Ok(())
}

/// COP-202 AC3: a recording set whose daemon CSV is missing or below nine of
/// ten fails the audit, even when the link CSV passes.
#[test]
fn floor_connection_audit_requires_the_daemon_recording() {
    let header = "trial,seed,snr_db,connected,elapsed_s,a_state,b_state,failure_reason\n";
    let csv = |connected: usize| {
        let mut csv = String::from(header);
        for index in 0..TRIALS {
            let seed = SEED_BASE + index as u64;
            if index < connected {
                csv.push_str(&format!(
                    "{index},{seed:#x},-6,true,36.0,CONNECTED,CONNECTED,-\n"
                ));
            } else {
                csv.push_str(&format!(
                    "{index},{seed:#x},-6,false,60.0,IDLE,IDLE,timeout\n"
                ));
            }
        }
        csv
    };
    let dir = std::env::temp_dir().join(format!(
        "cop202-audit-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("link.csv"), csv(10)).unwrap();
    let missing = audit_connect_dir(&dir).expect_err("a missing daemon CSV must fail");
    std::fs::write(dir.join("daemon.csv"), csv(8)).unwrap();
    let short = audit_connect_dir(&dir).expect_err("8/10 daemon trials must fail");
    std::fs::write(dir.join("daemon.csv"), csv(9)).unwrap();
    let passing = audit_connect_dir(&dir);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(missing.contains("daemon"), "{missing}");
    assert!(short.contains("daemon connect bar not met"), "{short}");
    assert_eq!(passing, Ok(()));
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

// Deterministic library session regressions below do not establish a connection probability.

use coppa_engine::LinkPhyMode;

// Tests need one real epoch; every subsequent deadline uses injected sample time.
#[allow(clippy::disallowed_methods)]
fn test_epoch() -> Instant {
    Instant::now()
}

/// Move actual audio in 200 ms blocks, with time derived from sample duration.
/// OFDM can be erased without erasing the independently detected floor.
fn pump(a: &mut Link, b: &mut Link, base: Instant, tick: &mut u64, seconds: u64, erase_ofdm: bool) {
    let (mut a_out, mut b_out) = (vec![0.0; 9600], vec![0.0; 9600]);
    for _ in 0..seconds * 5 {
        let now = base + Duration::from_millis(*tick * 200);
        *tick += 1;
        let (a_in, b_in) = (b_out.clone(), a_out.clone());
        a.process(now, &a_in, &mut a_out);
        b.process(now, &b_in, &mut b_out);
        if erase_ofdm {
            if a.phy_mode().0 != Some(LinkPhyMode::Floor) {
                a_out.fill(0.0);
            }
            if b.phy_mode().0 != Some(LinkPhyMode::Floor) {
                b_out.fill(0.0);
            }
        }
    }
}

#[test]
fn floor_pair_connects_and_delivers() {
    let base = test_epoch();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
    b.set_listening(true);
    a.connect("N0BBB", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 60, true);
    assert_eq!(a.state(), LinkState::Connected);
    assert_eq!(b.state(), LinkState::Connected);
    let payload = b"a message longer than the floor's nineteen application bytes";
    a.send(payload).unwrap();
    pump(&mut a, &mut b, base, &mut tick, 100, true);
    let mut received = [0; 100];
    let n = b.recv(&mut received);
    assert_eq!(&received[..n], payload);
    assert_eq!(a.pending_tx_bytes(), 0);
    assert_eq!(b.recv(&mut received), 0);
}

#[test]
fn floor_v1_peer_stays_ofdm() {
    let base = test_epoch();
    let mut config = LinkConfig::new("N0BBB");
    config.robust_floor = false;
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(config).unwrap();
    b.set_listening(true);
    a.connect("N0BBB", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 15, false);
    assert_eq!(a.state(), LinkState::Connected);
    assert_eq!(b.state(), LinkState::Connected);
    assert!(matches!(a.phy_mode().0, Some(LinkPhyMode::Ofdm { .. })));
    a.send(b"legacy").unwrap();
    pump(&mut a, &mut b, base, &mut tick, 15, false);
    let mut bytes = [0; 6];
    assert_eq!(b.recv(&mut bytes), 6);
    assert_eq!(&bytes, b"legacy");
}

#[test]
fn floor_phy_mode_reporting() {
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    assert_eq!(a.phy_mode(), (None, None));
    let now = test_epoch();
    a.connect("N0BBB", now).unwrap();
    a.process(now, &[], &mut [0.0; 9600]);
    assert!(matches!(a.phy_mode().0, Some(LinkPhyMode::Ofdm { .. })));
    assert_eq!(a.phy_mode().1, None);
    a.disconnect(now).unwrap();
    assert_eq!(a.phy_mode(), (None, None));
}

#[test]
fn floor_five_ack_probe() {
    for lose_probe in [false, true] {
        let base = test_epoch();
        let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
        let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
        b.set_listening(true);
        a.connect("N0BBB", base).unwrap();
        let mut tick = 0;
        pump(&mut a, &mut b, base, &mut tick, 60, true);
        assert_eq!(b.state(), LinkState::Connected);
        let payload: Vec<u8> = (0..19 * 7).map(|i| i as u8).collect();
        a.send(&payload).unwrap();
        let (mut a_out, mut b_out) = (vec![0.0; 9600], vec![0.0; 9600]);
        let mut saw_probe = false;
        let mut saw_return = false;
        let mut last_count = a.frames_transmitted();
        for _ in 0..240 * 5 {
            let now = base + Duration::from_millis(tick * 200);
            tick += 1;
            let (a_in, b_in) = (b_out.clone(), a_out.clone());
            a.process(now, &a_in, &mut a_out);
            b.process(now, &b_in, &mut b_out);
            if a.frames_transmitted() != last_count {
                last_count = a.frames_transmitted();
                if matches!(a.phy_mode().0, Some(LinkPhyMode::Ofdm { level: 1 })) {
                    saw_probe = true;
                }
                if saw_probe && a.phy_mode().0 == Some(LinkPhyMode::Floor) {
                    saw_return = true;
                }
            }
            if lose_probe {
                if a.phy_mode().0 != Some(LinkPhyMode::Floor) {
                    a_out.fill(0.0);
                }
                if b.phy_mode().0 != Some(LinkPhyMode::Floor) {
                    b_out.fill(0.0);
                }
            }
            if a.pending_tx_bytes() == 0 && b.received_len() == payload.len() {
                break;
            }
        }
        assert!(saw_probe, "five new ACKs must permit a probe");
        assert_eq!(saw_return, lose_probe);
        let mut received = vec![0; payload.len() + 1];
        assert_eq!(b.recv(&mut received), payload.len());
        assert_eq!(&received[..payload.len()], payload);
        assert_eq!(b.received_len(), 0);
        assert_eq!(a.pending_tx_bytes(), 0);
    }
}

#[test]
fn floor_simultaneous_calls() {
    let base = test_epoch();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
    // Outgoing calls must converge even when unsolicited incoming calls are disabled.
    a.connect("N0BBB", base).unwrap();
    b.connect("N0AAA", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 100, true);
    assert_eq!(a.state(), LinkState::Connected);
    assert_eq!(b.state(), LinkState::Connected);
    for link in [&mut a, &mut b] {
        assert_eq!(
            std::iter::from_fn(|| link.poll_event())
                .filter(|e| matches!(e, coppa_engine::LinkEvent::Connected { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn floor_simultaneous_calls_equal_retry_bucket() {
    let base = test_epoch();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0AAD")).unwrap();
    // Outgoing calls must converge even when unsolicited incoming calls are disabled.
    a.connect("N0AAD", base).unwrap();
    b.connect("N0AAA", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 150, true);
    assert_eq!(a.state(), LinkState::Connected);
    assert_eq!(b.state(), LinkState::Connected);
    for link in [&mut a, &mut b] {
        assert_eq!(
            std::iter::from_fn(|| link.poll_event())
                .filter(|e| matches!(e, coppa_engine::LinkEvent::Connected { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn floor_duplicate_handshake() {
    use coppa_protocol::{
        mac::{Callsign, MacFrameType, MacPdu},
        session::LinkCapabilities,
    };
    let base = test_epoch();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
    b.set_listening(true);
    a.connect("N0BBB", base).unwrap();
    let mut tick = 0;
    pump(&mut a, &mut b, base, &mut tick, 60, true);
    let caps = LinkCapabilities {
        robust_floor: true,
        ..Default::default()
    };
    let core = CoppaCore::new();
    // Replay complete, validated handshake audio after establishment.
    for (kind, dest, src, receiver) in [
        (MacFrameType::ConnectReq, "N0BBB", "N0AAA", &mut b),
        (MacFrameType::ConnectAck, "N0AAA", "N0BBB", &mut a),
    ] {
        let pdu = MacPdu::new(
            kind,
            Callsign::new(dest).unwrap(),
            Callsign::new(src).unwrap(),
            0,
            caps.to_bytes(),
        );
        let mut audio = core.floor().encode(&pdu.to_bytes()).unwrap();
        audio.extend(vec![0.0; 2 * SAMPLE_RATE]);
        for chunk in audio.chunks(9600) {
            receiver.process(
                base + Duration::from_millis(tick * 200),
                chunk,
                &mut [0.0; 9600],
            );
            tick += 1;
        }
        assert_eq!(receiver.state(), LinkState::Connected);
        assert_eq!(
            std::iter::from_fn(|| receiver.poll_event())
                .filter(|e| matches!(e, coppa_engine::LinkEvent::Connected { .. }))
                .count(),
            1
        );
        assert_eq!(receiver.received_len(), 0);
    }
}
