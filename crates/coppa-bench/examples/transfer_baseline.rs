//! V1 baseline transfer benchmark: payload-recovery vs channel/SNR over a multi-frame,
//! correlated-fading transfer. Establishes the collapse target SP2's V2 must beat.

use coppa_bench::scenario::ChannelSpec;
use coppa_bench::transfer::{run_transfer_scenario, transfer_to_csv, transfer_to_markdown, V1Phy};
use coppa_channel::watterson::WattersonPreset;
use std::fs;

fn main() {
    let level = 2u8; // BPSK 1/2
    let frames = 8usize; // multi-frame transfer; spans the Good coherence time
    let trials = 30usize;
    let snrs = [6.0f32, 12.0, 18.0, 24.0, 30.0];

    let channels: [(ChannelSpec, &str); 4] = [
        (ChannelSpec::Awgn, "AWGN"),
        (
            ChannelSpec::Watterson(WattersonPreset::Good),
            "Watterson Good",
        ),
        (
            ChannelSpec::Watterson(WattersonPreset::Moderate),
            "Watterson Moderate",
        ),
        (
            ChannelSpec::Watterson(WattersonPreset::Poor),
            "Watterson Poor",
        ),
    ];

    let phy = V1Phy::new(level, frames);
    let mut all = Vec::new();
    for (chan, name) in channels {
        eprintln!("Measuring transfer over {name} ...");
        let pts = run_transfer_scenario(&phy, "v1", level, chan, &snrs, trials, 0x00C0_FFEE);
        println!(
            "{}",
            transfer_to_markdown(&pts, &format!("V1 transfer — {name}"))
        );
        all.extend(pts);
    }
    fs::create_dir_all("results").expect("create results dir");
    fs::write("results/transfer_v1.csv", transfer_to_csv(&all)).expect("write csv");
    eprintln!("Wrote results/transfer_v1.csv");
}
