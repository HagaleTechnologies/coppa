//! `milstd` bench: MIL-STD-188-110 Table XVI-style operating points, mapped onto
//! Coppa's speed levels, with real pass/fail measurement against each point's
//! approximate reference SNR.
//!
//! Run: `cargo run -p coppa-bench --release --example milstd`
//!
//! ## Why an example, not a CLI subcommand (Task 8 design choice)
//!
//! This bench follows this crate's established pattern (`closed_loop_arq.rs`,
//! `task5_multi_codeword_transfer.rs`, `watterson_sanity_sweep.rs`, etc.): a
//! one-off bench tool lives in `examples/` as its own `fn main()`, run via
//! `cargo run -p coppa-bench --release --example <name>`. This avoids touching
//! `src/main.rs`'s existing flat `clap::Parser` sweep CLI (`cargo run -p
//! coppa-bench --release -- --trials 50`), which stays exactly as it was --
//! zero risk of regressing its stable, scripted usage. The alternative (folding
//! this into `src/main.rs` as a new `clap::Subcommand` variant) is also
//! defensible, but would mean rewriting a currently-flat, working CLI's
//! argument surface for a benefit (a single `coppa-bench` entry point) this
//! codebase's own prior art doesn't seem to have wanted, given how many
//! similar one-off benches already exist as separate example binaries.
//!
//! ## MIL-STD-188-110 Table XVI mapping -- READ THIS BEFORE TRUSTING THE NUMBERS
//!
//! This repo has no license to reproduce MIL-STD-188-110's tables verbatim,
//! and this bench is not an attempt at formal certification against the
//! standard. What follows is a **reasonable, openly-approximate** reference
//! table built from the standard's well-known serial-tone data-rate classes
//! (75/150/300/600/1200/2400/4800/9600 bps) and commonly-cited, rounded
//! SNR-vs-rate figures for those classes over CCIR/ITU-R Poor/Moderate/Good HF
//! channel simulations. Concretely:
//!
//! 1. **Level -> rate-class mapping** is by ascending order of Coppa's
//!    code-rate-weighted spectral efficiency (bits/symbol x code rate), matched
//!    ordinally to the standard's ascending rate-class ladder -- NOT by literal
//!    bps equivalence. Coppa's OFDM waveform (48 kHz, many parallel subcarriers)
//!    and MIL-STD's single-tone serial waveform are structurally different
//!    PHYs; a Coppa level's actual airtime-normalized goodput does not equal
//!    its assigned rate class's bps figure. Two levels (5: 8PSK 2/3, 6: 16QAM
//!    1/2) land on the same spectral efficiency (2.0 bits/symbol) and are
//!    mapped to the same rate class (1200 bps) -- there is no clean level
//!    between the 1200 and 2400 classes to split them across.
//! 2. **Reference SNR per rate class** is a simple, explicitly-approximate
//!    ladder: +3 dB per rate-class doubling, anchored so the 2400 bps class
//!    sits at 18 dB under the Poor preset (matching this task's own brief,
//!    which cites "2400 bps-class = level pairs at 18 dB poor" as an example
//!    operating point). This is NOT a value read off the standard's actual
//!    curves -- it is a rounded, whole-dB approximation consistent with
//!    commonly-published MIL-STD-188-110 SNR-vs-rate behavior (each rate
//!    doubling costing roughly 3 dB at a fixed target error rate). Moderate and
//!    Good operating points are then derived as -6 dB and -12 dB off the Poor
//!    anchor respectively -- again a rounded approximation, not standard data.
//! 3. **FER<->BER mapping assumption**: the standard's operating points target
//!    BER ~= 1e-5. For a per-frame union-bound approximation,
//!    FER ~= 1 - (1 - 1e-5)^n ~= n * 1e-5 for payload bit-count n. Coppa's
//!    levels carry ~450-1600 payload bits/frame (56-198 bytes), giving
//!    n*1e-5 ~= 0.5%-1.6%. Rather than compute a fractional per-level target
//!    (false precision), this bench uses a single, slightly conservative 2%
//!    FER threshold (Wilson-upper-bound) as the "BER<=1e-5-equivalent" pass
//!    line for every level.
//!
//! None of the above should be read as a literal reproduction of MIL-STD-188-110
//! Table XVI -- it is a documented, good-faith approximation for regression
//! tracking, built from public general knowledge of the standard's rate
//! classes and coarse published performance behavior, not from the standard's
//! text itself.

use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::{mode_for_level, ChannelSpec, Scenario};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::CoppaProfile;

/// One coppa level's assigned MIL-STD-188-110 rate class (see module doc for
/// the mapping rationale).
struct LevelMapping {
    level: u8,
    rate_class_bps: u32,
}

const LEVEL_MAPPINGS: &[LevelMapping] = &[
    LevelMapping {
        level: 1,
        rate_class_bps: 75,
    },
    LevelMapping {
        level: 2,
        rate_class_bps: 150,
    },
    LevelMapping {
        level: 3,
        rate_class_bps: 300,
    },
    LevelMapping {
        level: 4,
        rate_class_bps: 600,
    },
    LevelMapping {
        level: 5,
        rate_class_bps: 1200,
    },
    LevelMapping {
        level: 6,
        rate_class_bps: 1200,
    },
    LevelMapping {
        level: 7,
        rate_class_bps: 2400,
    },
    LevelMapping {
        level: 9,
        rate_class_bps: 4800,
    },
    LevelMapping {
        level: 10,
        rate_class_bps: 9600,
    },
];

/// Reference (Poor-preset) SNR for a rate class: +3 dB per doubling, anchored
/// at 2400 bps = 18 dB. See module doc, point 2.
fn poor_snr_for_rate_class(bps: u32) -> f32 {
    let doublings_from_2400 = (bps as f32 / 2400.0).log2();
    18.0 + 3.0 * doublings_from_2400
}

/// Per-preset SNR offset off the Poor anchor (module doc, point 2).
fn preset_offset_db(preset: WattersonPreset) -> f32 {
    match preset {
        WattersonPreset::Poor => 0.0,
        WattersonPreset::Moderate => 6.0,
        WattersonPreset::Good => 12.0,
    }
}

fn preset_name(preset: WattersonPreset) -> &'static str {
    match preset {
        WattersonPreset::Good => "good",
        WattersonPreset::Moderate => "moderate",
        WattersonPreset::Poor => "poor",
    }
}

/// Trials per operating point. 0/100 gives a Wilson upper bound of about
/// 3.6%, which still comfortably distinguishes "clears" from "fails" for this
/// bench's actual measured FERs (see the report: almost every point is either
/// near 0% or at least 50%, not borderline near the 2% threshold, so 100
/// trials/point doesn't trade away any real signal vs. a larger count). Each
/// of the 27 (level, preset) combinations is measured at TWO SNR points (the
/// reference point and a +12dB margin point -- see below), so the full run is
/// 27 x 2 x TRIALS real transmit/Watterson-fade/receive round trips; in
/// release mode this takes several minutes (Watterson fading synthesis and
/// LDPC decode both cost real CPU per trial) -- budget accordingly if you
/// increase TRIALS.
const TRIALS: usize = 100;

/// "BER<=1e-5-equivalent" FER pass threshold (module doc, point 3).
const FER_PASS_THRESHOLD: f64 = 0.02;

const BASE_SEED: u64 = 0x4D49_4C53_5444; // "MILSTD" in hex-ish

fn main() {
    println!("=== MIL-STD-188-110 Table XVI-style operating points (Coppa mapping) ===\n");
    println!("Mapping, SNR ladder, and FER<->BER assumptions: see this file's module doc.");
    println!(
        "Trials/point: {TRIALS}   FER pass threshold (Wilson upper bound): {:.1}%\n",
        FER_PASS_THRESHOLD * 100.0
    );
    println!(
        "| Level | Mode      | Rate class (bps) | Channel  | Op. SNR (dB) | Measured FER (95% CI) | Pass? | FER @ +12dB margin |"
    );
    println!(
        "|-------|-----------|-------------------|----------|--------------|------------------------|-------|--------------------|"
    );

    let mut total = 0usize;
    let mut passed = 0usize;
    let mut passed_with_margin = 0usize;

    for mapping in LEVEL_MAPPINGS {
        let mode = mode_for_level(mapping.level).expect("valid level");
        let base_snr = poor_snr_for_rate_class(mapping.rate_class_bps);
        for &preset in &[
            WattersonPreset::Good,
            WattersonPreset::Moderate,
            WattersonPreset::Poor,
        ] {
            let op_snr = base_snr - preset_offset_db(preset);
            // Also measure at op_snr+12dB: a generous margin, so a "fail" at the
            // literal reference point can be told apart from "fails even with
            // 12dB of headroom" -- a much more informative honest signal than a
            // bare pass/fail against one point (see this file's module doc and
            // this task's report for why the literal points fail almost across
            // the board on this codebase's current Watterson-channel path).
            let scenario = Scenario {
                level: mapping.level,
                channel: ChannelSpec::Watterson(preset),
                snr_db_points: vec![op_snr, op_snr + 12.0],
                trials: TRIALS,
                seed: BASE_SEED ^ ((mapping.level as u64) << 8) ^ (preset as u64),
                // MIL-STD-188-110 is an HF standard; force every level onto the HF
                // profile rather than this crate's default per-level VHF routing
                // (`select_profile` routes levels >=5 to `vhf_wide`, which is meant
                // for line-of-sight VHF use, not HF multipath -- comparing it
                // against an HF fading model would be an apples-to-oranges mismatch,
                // and its much-shorter cyclic prefix relative to the Watterson
                // delay spread caused every level >=5 trial to fail regardless of
                // SNR when this override was left off, confirmed by a direct A/B).
                profile_override: Some(CoppaProfile::hf_standard()),
                cfo_hz: 0.0,
                ssb: false,
            };
            let points = run_scenario(&scenario);
            let p = &points[0];
            let p_margin = &points[1];
            let pass = p.fer_hi <= FER_PASS_THRESHOLD;
            let pass_margin = p_margin.fer_hi <= FER_PASS_THRESHOLD;
            total += 1;
            if pass {
                passed += 1;
            }
            if pass_margin {
                passed_with_margin += 1;
            }
            println!(
                "| {:5} | {:9} | {:17} | {:8} | {:12.1} | {:6.2}% [{:.2}%, {:.2}%] | {:5} | {:6.2}% ({}) |",
                mapping.level,
                mode.name,
                mapping.rate_class_bps,
                preset_name(preset),
                op_snr,
                p.fer * 100.0,
                p.fer_lo * 100.0,
                p.fer_hi * 100.0,
                if pass { "PASS" } else { "fail" },
                p_margin.fer * 100.0,
                if pass_margin { "PASS" } else { "fail" },
            );
        }
    }

    println!(
        "\n{passed}/{total} operating points pass at the literal reference SNR (FER upper bound <= {:.1}%).",
        FER_PASS_THRESHOLD * 100.0
    );
    println!(
        "{passed_with_margin}/{total} pass even with a generous +12dB margin over the reference SNR."
    );
    println!(
        "\nSee this task's report for why the literal pass rate is low. The dominant cause, on \
         every channel including Good, is this ladder's own approximation: its reference SNRs \
         are borrowed and rounded from a DIFFERENT waveform's published operating points (see \
         module doc) and simply do not transfer onto Coppa's own measured thresholds -- e.g. \
         level 2's Good op. point here is -6.0 dB, but this codec's own independently-measured \
         Good-preset FER<=10% threshold for that mode is 12.0 dB (see BENCHMARKS.md), an 18 dB \
         gap that has nothing to do with any fading-specific bug. The already-documented \
         Watterson-Moderate/Poor channel-estimation gap (CLAUDE.md's \"Phase 2 channel \
         estimation\" / \"Turbo re-estimation\" known limitations) is a REAL, ADDITIONAL \
         contributing factor for the moderate/poor rows specifically -- but it is not a \
         blanket explanation for every failing row, including Good's, where no such known bug \
         applies at all."
    );
}
