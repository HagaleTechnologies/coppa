//! Exact replay of `closed_loop_arq`'s adaptive arms from its 18 fixed comparator cells
//! (COP-183).
//!
//! # What it replays
//!
//! [`replay`] drives the real [`coppa_ml::RateLoop`] and the real
//! [`crate::cp_arm::CpPolicy`] over a table of fixed-cell outcomes, in the same per-frame
//! order as `closed_loop_arq`'s `run_arm`:
//!
//! 1. `CpPolicy::mode_for_frame(f)` (CP-adaptive shape only);
//! 2. `RateLoop::level_for_next_transmission()`;
//! 3. look up cell `(mode, level)` at frame `f`, and record its slot;
//! 4. `CpPolicy::observe(f, level, delay_spread_ms)` (CP-adaptive shape only);
//! 5. `on_probe_result` for a probe, otherwise `on_ack` on a recommendation or `on_timeout`
//!    when the frame did not decode.
//!
//! # Why the replay is exact
//!
//! Every frame of `closed_loop_arq` is independent: frame `f` draws its channel from
//! `frame_seed(run_seed, f)` alone, and `run_frame` evicts the frame's IR-HARQ accumulator
//! after it decodes. So an adaptive arm's frame `f` at `(mode, level)` is the same
//! transmission, through the same channel, as fixed cell `(mode, level)`'s frame `f`. COP-183's
//! planning dumps confirmed it: arms A, B, B0, C and P matched their cells with 0 mismatches in
//! level, CP mode, delivery, receiver recommendation and delay spread over 7,500 frames per
//! profile, and replaying `RateLoop` and `CpPolicy` from the cells reproduced arms A, B and B0
//! frame for frame. `closed_loop_arq` re-checks this on every run (its `COP-183 replay-check`
//! lines), so a drift shows up as a non-zero mismatch count rather than a silently wrong sweep.
//!
//! # When it is not valid
//!
//! Only controllers that change the *sender's choice* of `(mode, level)` per frame can be
//! scored this way. Anything that changes the PHY, the receiver's recommendation, the channel,
//! or carries transceiver state from one frame into the next (IR-HARQ combining across frames,
//! a different header, a different payload) breaks the frame-independence the replay rests on
//! and needs a real run.

use crate::adaptive_goodput::FrameSlot;
use crate::cp_arm::{CpPolicy, CpSwitch};
use coppa_ml::{RateLoop, VALID_SPEED_LEVELS};
use coppa_protocol::cp_negotiator::CpMode;

/// What the receiver reported for one transmitted frame of one fixed cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellObs {
    /// Whether the payload decoded and matched.
    pub delivered: bool,
    /// The receiver's recommended speed level; `None` when the frame did not decode.
    pub recommended: Option<u8>,
    /// The receiver's measured delay spread; `Some` on any decode, `None` otherwise.
    pub delay_spread_ms: Option<f32>,
}

/// The 18 fixed comparator cells of one seed, in `run_seed`'s order:
/// `[LongCp L1..L9, ShortCp L1..L9]`. `slots[c][f]` and `obs[c][f]` describe cell `c`'s
/// frame `f`.
#[derive(Debug, Clone, Copy)]
pub struct CellTable<'a> {
    pub slots: &'a [Vec<FrameSlot>],
    pub obs: &'a [Vec<CellObs>],
}

impl CellTable<'_> {
    /// Frame count, or `None` unless the table holds 18 cells of one common length in both
    /// halves.
    fn frames(&self) -> Option<usize> {
        let cells = 2 * VALID_SPEED_LEVELS.len();
        if self.slots.len() != cells || self.obs.len() != cells {
            return None;
        }
        let n = self.slots[0].len();
        if self.slots.iter().any(|c| c.len() != n) || self.obs.iter().any(|c| c.len() != n) {
            return None;
        }
        Some(n)
    }
}

/// Index of cell `(mode, level)` in a [`CellTable`]. `None` for a level outside
/// `VALID_SPEED_LEVELS`.
fn cell_index(mode: CpMode, level: u8) -> Option<usize> {
    let rank = VALID_SPEED_LEVELS.iter().position(|&l| l == level)?;
    Some(match mode {
        CpMode::LongCp => rank,
        CpMode::ShortCp => VALID_SPEED_LEVELS.len() + rank,
    })
}

/// The `RateLoop` parameters a replay runs. The level set is always `VALID_SPEED_LEVELS` and
/// the start level is always 1, as in `closed_loop_arq` and coppad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerSpec {
    pub raise_dwell: u8,
    /// `0` disables probing.
    pub probe_interval: u32,
    pub probe_offset: usize,
}

impl ControllerSpec {
    /// `closed_loop_arq`'s controller before COP-183: dwell 5, probing (2, 1).
    pub const BASELINE: Self = Self {
        raise_dwell: 5,
        probe_interval: 2,
        probe_offset: 1,
    };
    /// COP-183's one candidate: dwell 1, probing unchanged at (2, 1).
    pub const CANDIDATE: Self = Self {
        raise_dwell: 1,
        probe_interval: 2,
        probe_offset: 1,
    };
    /// coppad's default shape before COP-183: dwell 5, probing off
    /// (`rate_loop_probe_interval = 0`).
    pub const COPPAD_DEFAULT_BASELINE: Self = Self {
        raise_dwell: 5,
        probe_interval: 0,
        probe_offset: 0,
    };
    /// coppad's default shape with the candidate dwell: dwell 1, probing off.
    pub const COPPAD_DEFAULT_CANDIDATE: Self = Self {
        raise_dwell: 1,
        probe_interval: 0,
        probe_offset: 0,
    };

    /// A fresh controller with these parameters. `with_probing(0, _)` is identical to a
    /// controller with no probing configured.
    ///
    /// # Panics
    /// Only if the built-in `VALID_SPEED_LEVELS` table stops being non-empty and strictly
    /// ascending.
    pub fn build(&self) -> RateLoop {
        RateLoop::new(VALID_SPEED_LEVELS.to_vec(), self.raise_dwell, 1)
            .expect("built-in speed levels are non-empty and strictly ascending")
            .with_probing(self.probe_interval, self.probe_offset)
    }
}

/// The CP behaviour of a replayed arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpShape {
    /// One CP mode for the whole run (arms A and C).
    Fixed(CpMode),
    /// `CpPolicy`-driven switching at this latency, booting on long CP (arms B and B0).
    Adaptive { latency_frames: usize },
}

/// What a replay produced: the slot each frame transmitted, and every CP switch decided.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayRun {
    pub slots: Vec<FrameSlot>,
    pub switches: Vec<CpSwitch>,
}

/// Replay one adaptive arm over `table`. See the module doc for the call order and the
/// validity limit.
///
/// Returns `None` for a table that is not 18 cells of equal length (slots and observations
/// alike), or if the controller ever asks for a level outside `VALID_SPEED_LEVELS`.
pub fn replay(table: &CellTable<'_>, spec: ControllerSpec, cp: CpShape) -> Option<ReplayRun> {
    let n = table.frames()?;
    let mut rate = spec.build();
    let (mut policy, fixed_mode) = match cp {
        CpShape::Fixed(mode) => (None, mode),
        CpShape::Adaptive { latency_frames } => {
            (Some(CpPolicy::new(latency_frames)), CpMode::LongCp)
        }
    };
    let mut slots = Vec::with_capacity(n);

    for f in 0..n {
        let mode = match policy.as_mut() {
            Some(p) => p.mode_for_frame(f),
            None => fixed_mode,
        };
        let (level, is_probe) = rate.level_for_next_transmission();
        let c = cell_index(mode, level)?;
        let slot = table.slots[c][f];
        let obs = table.obs[c][f];
        debug_assert_eq!(
            slot.level, level,
            "cell {c} frame {f} holds the wrong level"
        );
        slots.push(slot);

        if let Some(p) = policy.as_mut() {
            p.observe(f, level, obs.delay_spread_ms);
        }

        if is_probe {
            rate.on_probe_result(level, obs.delivered);
        } else {
            match obs.recommended {
                Some(r) => rate.on_ack(r, obs.delivered),
                None => rate.on_timeout(),
            }
        }
    }

    Some(ReplayRun {
        slots,
        switches: policy.map(|p| p.switches().to_vec()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cp_arm::SWITCH_LATENCY_FRAMES;
    use coppa_codec::ofdm::CoppaProfile;

    const N_LEVELS: usize = VALID_SPEED_LEVELS.len();

    fn profile(mode: CpMode) -> CoppaProfile {
        match mode {
            CpMode::LongCp => CoppaProfile::hf_standard(),
            CpMode::ShortCp => CoppaProfile::hf_standard_short_cp(),
        }
    }

    /// An 18-cell table of `n` frames whose cell `(mode, level)` frame `f` is `obs(mode, level, f)`.
    fn table(
        n: usize,
        obs: impl Fn(CpMode, u8, usize) -> CellObs,
    ) -> (Vec<Vec<FrameSlot>>, Vec<Vec<CellObs>>) {
        let mut slots = Vec::new();
        let mut cells = Vec::new();
        for mode in [CpMode::LongCp, CpMode::ShortCp] {
            for &level in VALID_SPEED_LEVELS.iter() {
                let col: Vec<CellObs> = (0..n).map(|f| obs(mode, level, f)).collect();
                slots.push(
                    col.iter()
                        .map(|o| FrameSlot::for_level(level, &profile(mode), o.delivered).unwrap())
                        .collect(),
                );
                cells.push(col);
            }
        }
        (slots, cells)
    }

    fn ok(rec: u8, spread: f32) -> CellObs {
        CellObs {
            delivered: true,
            recommended: Some(rec),
            delay_spread_ms: Some(spread),
        }
    }

    fn levels(run: &ReplayRun) -> Vec<u8> {
        run.slots.iter().map(|s| s.level).collect()
    }

    #[test]
    fn replay_rejects_a_malformed_table() {
        let (slots, obs) = table(10, |_, _, _| ok(1, 3.0));
        let spec = ControllerSpec::BASELINE;
        let cp = CpShape::Fixed(CpMode::LongCp);
        // The well-formed table replays.
        assert!(replay(
            &CellTable {
                slots: &slots,
                obs: &obs
            },
            spec,
            cp
        )
        .is_some());
        // 17 cells.
        let short = CellTable {
            slots: &slots[..2 * N_LEVELS - 1],
            obs: &obs[..2 * N_LEVELS - 1],
        };
        assert!(replay(&short, spec, cp).is_none());
        // One cell's observations shorter than its slots.
        let mut bad_obs = obs.clone();
        bad_obs[4].pop();
        let t = CellTable {
            slots: &slots,
            obs: &bad_obs,
        };
        assert!(replay(&t, spec, cp).is_none());
        // One cell's slots shorter than every other cell's.
        let mut bad_slots = slots.clone();
        bad_slots[11].pop();
        let t = CellTable {
            slots: &bad_slots,
            obs: &obs,
        };
        assert!(replay(&t, spec, cp).is_none());
    }

    #[test]
    fn fixed_cp_replay_raises_after_dwell_higher_recommendations() {
        let n = 50;
        let (slots, obs) = table(n, |_, _, _| ok(9, 3.0));
        let t = CellTable {
            slots: &slots,
            obs: &obs,
        };
        let cp = CpShape::Fixed(CpMode::LongCp);

        let d5 = replay(&t, ControllerSpec::COPPAD_DEFAULT_BASELINE, cp).unwrap();
        let want5: Vec<u8> = (0..n).map(|f| ((f / 5) as u8 + 1).min(9)).collect();
        assert_eq!(levels(&d5), want5);

        let d1 = replay(&t, ControllerSpec::COPPAD_DEFAULT_CANDIDATE, cp).unwrap();
        let want1: Vec<u8> = (0..n).map(|f| (f as u8 + 1).min(9)).collect();
        assert_eq!(levels(&d1), want1);
        assert_eq!(&levels(&d1)[..10], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 9]);

        // Every slot is the long-CP cell's slot for that frame.
        for (f, s) in d1.slots.iter().enumerate() {
            assert_eq!(*s, slots[cell_index(CpMode::LongCp, s.level).unwrap()][f]);
        }
        assert!(d1.switches.is_empty());
    }

    #[test]
    fn probe_frames_use_on_probe_result_and_never_on_ack() {
        let fail = CellObs {
            delivered: false,
            recommended: Some(9),
            delay_spread_ms: Some(3.0),
        };
        let cp = CpShape::Fixed(CpMode::LongCp);

        // The plan's fixture: level 1 delivers and recommends 1, level 2 fails. Every second
        // frame is a level-2 probe and the level never leaves 1.
        let (slots, obs) = table(40, |_, l, _| if l == 1 { ok(1, 3.0) } else { fail });
        let t = CellTable {
            slots: &slots,
            obs: &obs,
        };
        let run = replay(&t, ControllerSpec::BASELINE, cp).unwrap();
        for (f, s) in run.slots.iter().enumerate() {
            let want = if f % 2 == 1 { 2 } else { 1 };
            assert_eq!(s.level, want, "frame {f}");
            assert_eq!(s.delivered, want == 1, "frame {f}");
        }

        // A fixture that tells `on_probe_result` from `on_ack`: dwell 1 climbs to level 2 on
        // frame 0, then every odd frame probes level 3, which fails while recommending 9. A
        // failed probe is a no-op, so the even frames stay on level 2. Feeding the probe to
        // `on_ack(9, false)` or `on_timeout` would drop them to level 1 instead.
        let (slots, obs) = table(40, |_, l, _| match l {
            1 | 2 => ok(2, 3.0),
            _ => fail,
        });
        let t = CellTable {
            slots: &slots,
            obs: &obs,
        };
        let run = replay(&t, ControllerSpec::CANDIDATE, cp).unwrap();
        let mut want = vec![1u8];
        for f in 1..40 {
            want.push(if f % 2 == 1 { 3 } else { 2 });
        }
        assert_eq!(levels(&run), want);

        // The same sequence from a RateLoop driven by hand the way `run_arm` drives it.
        let mut r = ControllerSpec::CANDIDATE.build();
        for (f, &w) in want.iter().enumerate() {
            let (level, is_probe) = r.level_for_next_transmission();
            assert_eq!(level, w, "frame {f}");
            assert_eq!(is_probe, f % 2 == 1, "frame {f}");
            if is_probe {
                r.on_probe_result(level, false);
            } else {
                r.on_ack(2, true);
            }
        }
    }

    #[test]
    fn undecoded_frames_drop_one_level_through_on_timeout() {
        let undecoded = CellObs {
            delivered: false,
            recommended: None,
            delay_spread_ms: None,
        };
        let (slots, obs) = table(12, |_, l, _| if l < 4 { ok(9, 3.0) } else { undecoded });
        let t = CellTable {
            slots: &slots,
            obs: &obs,
        };
        let run = replay(
            &t,
            ControllerSpec::COPPAD_DEFAULT_CANDIDATE,
            CpShape::Fixed(CpMode::LongCp),
        )
        .unwrap();
        // 1, 2, 3 climb at dwell 1; level 4 does not decode and drops one level to 3; level 3
        // recommends 9 again, so the run alternates 4, 3 from there.
        assert_eq!(levels(&run), vec![1, 2, 3, 4, 3, 4, 3, 4, 3, 4, 3, 4]);
        assert!(run.slots.iter().all(|s| s.delivered == (s.level < 4)));
    }

    #[test]
    fn adaptive_cp_replay_switches_where_cp_policy_does() {
        let n = 20;
        // Long-CP cells read a calm 0.5 ms channel, short-CP cells the same; every cell holds
        // level 1 (recommends 1), so the deciding frame is on level 1.
        let (slots, obs) = table(n, |_, _, _| ok(1, 0.5));
        let t = CellTable {
            slots: &slots,
            obs: &obs,
        };
        let run = replay(
            &t,
            ControllerSpec::COPPAD_DEFAULT_BASELINE,
            CpShape::Adaptive {
                latency_frames: SWITCH_LATENCY_FRAMES,
            },
        )
        .unwrap();
        assert_eq!(
            run.switches,
            vec![CpSwitch {
                from: CpMode::LongCp,
                to: CpMode::ShortCp,
                decided_at: 3,
                effective_from: 9,
                level: 1,
            }]
        );
        let long = cell_index(CpMode::LongCp, 1).unwrap();
        let short = cell_index(CpMode::ShortCp, 1).unwrap();
        for (f, s) in run.slots.iter().enumerate() {
            let want = if f < 9 {
                slots[long][f]
            } else {
                slots[short][f]
            };
            assert_eq!(*s, want, "frame {f}");
        }
        // Long and short slots really differ, so the check above is not vacuous.
        assert_ne!(slots[long][0].airtime_s, slots[short][0].airtime_s);

        // A real CpPolicy fed the same frames decides the same switch.
        let mut p = CpPolicy::new(SWITCH_LATENCY_FRAMES);
        for f in 0..n {
            p.mode_for_frame(f);
            p.observe(f, 1, Some(0.5));
        }
        assert_eq!(p.switches(), &run.switches[..]);
    }

    #[test]
    fn controller_spec_baseline_matches_the_bench_controller() {
        let mut spec = ControllerSpec::BASELINE.build();
        let mut bench = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 5, 1)
            .unwrap()
            .with_probing(2, 1);
        // A scripted mix: higher recommendations, holds, a drop, a failure, a timeout, and
        // probes that succeed and fail.
        for f in 0..40usize {
            let a = spec.level_for_next_transmission();
            let b = bench.level_for_next_transmission();
            assert_eq!(a, b, "frame {f}");
            let (level, is_probe) = a;
            let delivered = f % 7 != 3;
            if is_probe {
                spec.on_probe_result(level, f % 3 == 0);
                bench.on_probe_result(level, f % 3 == 0);
            } else if f % 11 == 10 {
                spec.on_timeout();
                bench.on_timeout();
            } else {
                let rec = [9u8, 9, 9, 4, 9, 2, 9, 9][f % 8];
                spec.on_ack(rec, delivered);
                bench.on_ack(rec, delivered);
            }
            assert_eq!(spec.current_level(), bench.current_level(), "frame {f}");
        }
        assert_eq!(ControllerSpec::CANDIDATE.raise_dwell, 1);
        assert_eq!(
            (
                ControllerSpec::CANDIDATE.probe_interval,
                ControllerSpec::CANDIDATE.probe_offset
            ),
            (2, 1)
        );
        assert_eq!(ControllerSpec::COPPAD_DEFAULT_BASELINE.probe_interval, 0);
        assert_eq!(ControllerSpec::COPPAD_DEFAULT_CANDIDATE.raise_dwell, 1);
    }
}
