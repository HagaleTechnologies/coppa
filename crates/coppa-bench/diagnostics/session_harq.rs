//! COP-168: calibrated, paired Chase versus all six incremental-RV schedules.
//! Run with `cargo run --release -p coppa-bench --example session_harq`.
use coppa_bench::{metrics::wilson_ci95, session_harq::SessionHarqTrial};
use coppa_protocol::modem::transceiver::HarqPolicy;
use std::fmt::Write;
use std::time::Instant;

const ORDERS: [[u8; 3]; 7] = [
    [0, 0, 0],
    [2, 3, 1],
    [2, 1, 3],
    [3, 2, 1],
    [3, 1, 2],
    [1, 2, 3],
    [1, 3, 2],
];

fn calibration(phy: &SessionHarqTrial, level: u8) -> f32 {
    // Binary search the 0.5 dB lattice, then retain the closest measured point.
    // Every reported point uses 400 independent frames, never retained HARQ.
    let (mut low, mut high) = (-20i32, 100i32);
    let mut best = (f64::INFINITY, 0.0);
    while low <= high {
        let mid = (low + high) / 2;
        let snr = mid as f32 * 0.5;
        let mut failed = 0;
        for trial in 0..400 {
            let packet = phy.packet(0xCA11_0000 + trial, 0);
            phy.receiver.harq_evict(0);
            failed += usize::from(!phy.receive(&packet, snr, 0xCA11_0000 + trial, 0));
        }
        let fer = failed as f64 / 400.0;
        println!("CAL level={level} snr={snr:.1} failed={failed}/400 FER={fer:.4}");
        if (fer - 0.30).abs() < best.0 {
            best = ((fer - 0.30).abs(), snr);
        }
        if (0.25..=0.35).contains(&fer) {
            return snr;
        }
        if fer > 0.30 {
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }
    println!(
        "CAL level={level} unattainable_25_to_35_percent=true closest_snr={}",
        best.1
    );
    best.1
}

fn trial(phy: &SessionHarqTrial, snr: f32, index: u64) -> Option<[[bool; 3]; 7]> {
    let first = phy.packet(index, 0);
    phy.receiver.harq_evict(0);
    if phy.receive(&first, snr, index, 0) {
        return None;
    }
    let packets = phy.packets(index);
    let mut success = [[false; 3]; 7];
    for (arm, order) in ORDERS.iter().enumerate() {
        phy.receiver.harq_evict(0);
        assert!(
            !phy.receive(&packets[0], snr, index, 0),
            "paired RV0 must be deterministic"
        );
        let mut done = false;
        for (retry, &rv) in order.iter().enumerate() {
            if !done {
                done = phy.receive(&packets[rv as usize], snr, index, retry as u64 + 1);
            }
            success[arm][retry] = done;
        }
    }
    Some(success)
}

fn measure(level: u8, cw: u8, snr: f32) -> [u8; 3] {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    let next = AtomicU64::new(0);
    let failed = AtomicUsize::new(0);
    // Multi-codeword frames cost up to eight decodes per attempt. Spread that
    // row across workers as well; sort by seed before applying the stopping rule
    // so results are independent of thread scheduling.
    let workers = if cw > 1 { 6 } else { 1 };
    let mut outcomes = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            handles.push(scope.spawn(|| {
                let phy = SessionHarqTrial::new(level, cw, true);
                phy.receiver.set_harq_policy(HarqPolicy {
                    key_mask: 0x0F,
                    rv0_starts_fresh: false,
                    multi_codeword: true,
                });
                let mut results = Vec::new();
                while failed.load(Ordering::Relaxed) < 400 {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= 2000 {
                        break;
                    }
                    let result = trial(&phy, snr, index);
                    if result.is_some() {
                        let completed = failed.fetch_add(1, Ordering::Relaxed) + 1;
                        if completed % 100 == 0 {
                            println!("PROGRESS level={level} cw={cw} failed={completed} latest_trial={index}");
                        }
                    }
                    results.push((index, result));
                }
                results
            }));
        }
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    outcomes.sort_unstable_by_key(|(index, _)| *index);
    let mut rescues = [[0usize; 3]; 7];
    let mut discordant = [[[0usize; 2]; 3]; 7];
    let (mut trials, mut failures) = (0, 0);
    for (_, success) in outcomes {
        trials += 1;
        let Some(success) = success else {
            continue;
        };
        failures += 1;
        for arm in 0..7 {
            for retry in 0..3 {
                rescues[arm][retry] += usize::from(success[arm][retry]);
                discordant[arm][retry][0] += usize::from(success[arm][retry] && !success[0][retry]);
                discordant[arm][retry][1] += usize::from(!success[arm][retry] && success[0][retry]);
            }
        }
        if failures >= 400 {
            break;
        }
    }
    for arm in 0..7 {
        let mut line = format!("RESULT level={level} cw={cw} snr={snr:.1} trials={trials} failures={failures} order={:?}", ORDERS[arm]);
        for retry in 0..3 {
            let k = rescues[arm][retry];
            let (lo, hi) = wilson_ci95(k, failures);
            write!(
                line,
                " after_{}={k} CI95=[{lo:.4},{hi:.4}] discordant={:?}",
                retry + 2,
                discordant[arm][retry]
            )
            .unwrap();
        }
        println!("{line}");
    }
    // First rescue count dominates. A tie within one frame uses rescue after
    // the next retry, then Kendall distance to the 3GPP order.
    // Equal distances retain the stable order of ORDERS.
    let max_first = rescues[1..].iter().map(|r| r[0]).max().unwrap();
    let selected = rescues
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, counts)| counts[0] + 1 >= max_first)
        .min_by_key(|(arm, counts)| (std::cmp::Reverse(counts[1]), distance(ORDERS[*arm]), *arm))
        .unwrap()
        .0;
    let margin =
        100.0 * (rescues[selected][0] as f64 - rescues[0][0] as f64) / failures.max(1) as f64;
    println!(
        "SELECT level={level} cw={cw} order={:?} first_rescue_margin_pp={margin:.3}",
        ORDERS[selected]
    );
    ORDERS[selected]
}

/// Number of adjacent swaps from 3GPP's [2,3,1] order, i.e. Kendall distance.
fn distance(order: [u8; 3]) -> usize {
    let rank = |rv| match rv {
        2 => 0,
        3 => 1,
        _ => 2,
    };
    (0..3)
        .map(|i| {
            ((i + 1)..3)
                .filter(|&j| rank(order[i]) > rank(order[j]))
                .count()
        })
        .sum()
}

fn main() {
    let start = Instant::now();
    println!("COP-168 session_harq release={} calibration=400 max_trials=2000 stop_failures=400 Watterson=Moderate tail=1200 noise_reference=3kHz", !cfg!(debug_assertions));
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for level in 1..=8 {
            handles.push(scope.spawn(move || {
                let phy = SessionHarqTrial::new(level, 1, true);
                let snr = calibration(&phy, level);
                let order = measure(level, 1, snr);
                if level == 4 {
                    measure(level, 8, snr);
                }
                (level, order)
            }));
        }
        for h in handles {
            let (level, order) = h.join().unwrap();
            println!(
                "SCHEDULE level={level} row=[0, {}, {}, {}]",
                order[0], order[1], order[2]
            );
        }
    });
    println!("wall_seconds={:.3}", start.elapsed().as_secs_f64());
}
