> Dated benchmark log. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-183: Rate-loop re-measure and raise-dwell candidate

### Scope

This is an in-process simulation with `crates/coppa-bench/diagnostics/closed_loop_arq.rs`. The rate loop gets immediate per-frame feedback, stop-and-wait with window 1. It is **not** a windowed result. COP-168 put the windowed-session primitives on main (2bf36d5), but coppad still runs stop-and-wait (`SESSION_ARQ_WINDOW = 1`, `windowed_arq: false`) and calls `RateLoop::on_ack` once per single-frame ACK. The burst sender and per-burst ACK wait on COP-184. No bench gives the rate loop windowed feedback, so every number here is a stop-and-wait number. There is no on-air or two-radio evidence.

### Pre-registration

Fixed in the COP-183 plan before any code was written and before any acceptance-seed result existed. Restated here unchanged.

- **Direction.** One candidate from the ticket's list: faster controller convergence.
- **The one frozen parameter reopened.** `RateLoop`'s raise dwell, 5 to 1 (`raise_dwell` in `RateLoop::new`). COP-152 froze "controller dwell". Nothing else in `RateLoop` changes: probe interval and offset stay (2, 1) on the bench and off in coppad, and the stall-gate code stays. Side effect, stated up front because it carries most of the effect: at dwell 1 every ACK leaves `raise_run` at zero, so the probe stall gate never fires and, with probing (2, 1), the controller probes one level up on every second frame.
- **Proof budget.** The ticket's 3.5 h compute cap. Estimate: about 1.6 h of implement-phase compute on top of 33 min of planning-phase screening.
- **Development seeds.** Seeds 5 to 9 (`COPPA_CL_SEED_OFFSET=5`, frame seeds 5120 to 9515), disjoint from the acceptance seeds 0 to 4. The candidate was screened and frozen on them in planning. The acceptance seeds were not run with the candidate until this implement run.
- **Bar status (unchanged from COP-152).** C1, robust historical: robust, seed 0, 300 frames, long-CP adaptive arm slot counts against the best fixed long-CP bit count and the long-CP per-frame oracle bit count (`legacy_rate_bars`: `adaptive > fixed` and `5*adaptive >= 4*oracle`). C2, standard production: standard, seeds 0 to 4, 300 frames, arm B x1 against the joint 18-cell best fixed and oracle, ratios of five-seed sums (`standard_rate_bars`: `> 1.0` and `>= 0.8`). The bar is met only when all four predicates hold for the shipped controller. Robust arm B x1 against the joint comparator is descriptive.
- **Adoption gates.** All four must hold on both profiles, on the acceptance seeds (0 to 4, 300 frames). Baseline is the dwell-5 arms of the same run, candidate the dwell-1 arms.
  - G1: arm B-d1 x1 summed over the five seeds is greater than arm B x1's sum, and arm B-d1 x1 is greater than arm B x1 on each seed.
  - G2: pooled arm B-d1 x1 goodput in the Watterson Good segment and in the Watterson Poor segment is each at least the baseline's. Pooled means segment bits over segment data air plus the switch air charged to the segment of the deciding frame.
  - G3: arm A-d1 goodput summed over seeds is at least arm A's.
  - G4: coppad's default shape (long CP fixed, probing off), replayed, at dwell 1 summed over seeds is at least the dwell-5 sum.

  C1 is not a gate: it counts bits per frame slot and cannot credit the shorter airtime of faster levels. It is reported for both controllers, and any drop is stated. The rule is the tested function `coppa_bench::adaptive_goodput::adoption_gates`.
- **Stop conditions.** A non-zero `COP-183 replay-check total_mismatches` voids the replay-derived numbers (the sweep and G4) and rejects the candidate. Baseline drift from COP-152's tables does not stop the run (G1 to G4 compare arms within one run); it is recorded with its cause. A development cross-check that misses the plan's reference values by more than a relative 1e-6 stops the run before the acceptance seeds are read. Cumulative candidate-proof compute past 3.5 h is recorded as "not proven within budget", a rejection.

### Commands and environment

The branch is based on 2909f04, COP-183's branch point. `origin/main` was 201c629 when these runs started. `git diff --stat 2909f04 201c629 -- crates/coppa-ml crates/coppa-bench crates/coppa-codec crates/coppa-channel crates/coppa-protocol/src/modem/transceiver.rs crates/coppa-protocol/src/arq.rs` is empty, so main's six newer commits do not touch anything this bench runs. The bench source for these runs is 2909f04 plus this ticket's replay library, adoption gates and candidate arms (commit 364be57 on the COP-183 branch). Two later review fixes change only text this bench prints on an offset (development) run and on a run whose replay is not exact; no printed number moves. The container had a 6-CPU quota (`cpu.max = 600000 100000`).

```sh
cargo build --release -p coppa-bench --example closed_loop_arq --example session
./target/release/examples/closed_loop_arq robust                          # acceptance seeds 0-4, 300 frames
./target/release/examples/closed_loop_arq standard
COPPA_CL_SEED_OFFSET=5 ./target/release/examples/closed_loop_arq robust   # development seeds 5-9
COPPA_CL_SEED_OFFSET=5 ./target/release/examples/closed_loop_arq standard
./target/release/examples/session
COPPA_CL_FRAMES=24 COPPA_CL_SEEDS=1 ./target/release/examples/closed_loop_arq robust  # smoke, before and after
```

Wall times, all four full runs and the session bench started together on the shared 6-CPU quota (the development runs were paused for part of the acceptance runs so the decisional runs finished first): acceptance robust 20 min 24 s, acceptance standard 19 min 48 s, development robust did not finish in this session, development standard did not finish in this session, session did not finish in this session. Each seed ran 26 arms: 18 fixed cells, A, C, C', B, B0, P, A-d1 and B-d1.

The 24-frame smoke run's stdout before the change and after it are identical once the inserted `=== COP-183` blocks are removed; the prefix up to the first `=== COP-183` header is byte-identical. Every `COP-183` predicate line on the smoke run prints `None`, and it prints `COP-183 replay-check total_mismatches=0`.

### Re-measure: main's controller (raise dwell 5) on the acceptance seeds

Main's controller reproduces COP-152's baseline exactly. Every per-seed arm B row, every joint comparator and the robust seed-0 slot counts below equal `docs/benchmarks/2026-11-06-cop-152-bounded-prefix-residual.md`'s baseline tables to every printed digit. There is no drift.

Robust historical comparison (C1), seed 0, long-CP slot counts: adaptive 330144, fixed 358848, oracle 429256. Ratios 0.920010700910 and 0.769107478987; predicates `330144 > 358848` = False and `5 * 330144 >= 4 * 429256` = False. The bench's line:

```text
COP-152 legacy seed=0 adaptive=330144 fixed=358848 oracle=429256 bars=Some((false, (None = non-decisional)
```

Standard production comparison (C2): arm B x1 sums of per-seed goodputs are [11260.240734940584, 12636.411899313538, 18309.26976867539], adaptive/fixed/oracle. Ratios of means 0.891094784236 and 0.615002175248; >1.0 = False, >=0.8 = False. The bench's line:

```text
COP-152 standard arm-B-x1 operands=[[2150.6073670118085, 2491.3623188405863, 3618.2186707233354], [2158.8581270498617, 2519.9694889397483, 3658.3252238956748], [2131.311569690863, 2498.2456140350955, 3662.812316559057], [2326.168218202225, 2563.4172387490544, 3665.815170102382], [2493.295452985826, 2563.4172387490544, 3704.09838739494]] bars=Some((false, false)) (None = non-decisional)
```

Robust arm B x1 against the joint comparators (descriptive): [9005.422717961264, 11321.242236024804, 16287.8424295188], ratios 0.795444751576 and 0.552892303381.

Main's baseline does not beat best-fixed on either profile, so the ticket's "close early" branch does not apply.

### Session bench

`crates/coppa-bench/diagnostics/session.rs` runs level 2 fixed, window 8, five 10-minute sessions per Watterson preset. It has no `RateLoop` and no oracle, so it yields no adaptive/best-fixed or adaptive/oracle ratio and is not rate-loop evidence. It is recorded because the ticket asked for it.

This session's rerun did not finish before the phase deadline; 15 session trials had completed when the log was written, and they are in the complete outputs below. The research phase ran the same bench at 2909f04 to completion: Good 2/5 sessions without a drop (the COP-5 floor of 2/5 is met), Moderate 0/5, Poor 0/5; the COP-119 arms report stop-and-wait ideal-ACK Good 3/5, Moderate 4/5, Poor 3/5, and RS(12,6) 5/5 on every preset.

### Development-seed screen (seeds 5 to 9)

The development runs did not finish before this log was written; the planning-phase screen (plan Appendix A) is the only development evidence. See the implement report.

Stall-gate attribution from planning (development seeds, probing (2,1), stall gate removed): B/bf for dwell 1, 2 and 5 was 1.0772, 1.0403 and 1.0109 on standard and 1.0396, 0.9883 and 0.9866 on robust. Dwell 1 with or without the gate is identical, because at dwell 1 the gate never fires. Most of the candidate's gain comes through the probing cadence the dwell change unlocks.

### Candidate on the acceptance seeds (seeds 0 to 4)

#### Robust (synthetic short-CP twin; joint comparison descriptive)

| Controller | Seed | B delivered bits | Data air s | Switch air s | B x1 bps | Joint best fixed bps | Joint oracle bps | Arm A bps |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| baseline (dwell 5) | 0 | 324096 | 176.55524999999975 | 7.60975 | 1759.8132109792873 | 2240.662525879909 | 3258.1447162773043 | 1484.8777196829135 |
| baseline (dwell 5) | 1 | 341520 | 163.97425000000015 | 7.60975 | 1990.3953748601252 | 2230.8350586611377 | 3218.2865690479243 | 1515.804171393664 |
| baseline (dwell 5) | 2 | 323128 | 190.79225000000002 | 7.60975 | 1628.652936966361 | 2329.109730848853 | 3303.837074380825 | 1519.5730089155256 |
| baseline (dwell 5) | 3 | 334648 | 173.22025000000014 | 7.60975 | 1850.6221312835246 | 2299.6273291925386 | 3259.7881633716934 | 1536.128479910251 |
| baseline (dwell 5) | 4 | 329680 | 178.02724999999987 | 7.60975 | 1775.9390638719665 | 2221.007591442366 | 3247.7859064410522 | 1508.2007671666627 |
| candidate (dwell 1) | 0 | 312152 | 135.9635000000002 | 3.11875 | 2244.369788380613 | 2240.662525879909 | 3258.1447162773043 | 1968.0600161808281 |
| candidate (dwell 1) | 1 | 309752 | 130.30550000000022 | 3.11875 | 2321.5569883285793 | 2230.8350586611377 | 3218.2865690479243 | 1958.6316475205308 |
| candidate (dwell 1) | 2 | 322776 | 132.49050000000022 | 3.11875 | 2380.1916167223067 | 2329.109730848853 | 3303.837074380825 | 2037.3711574544707 |
| candidate (dwell 1) | 3 | 310752 | 128.46550000000025 | 3.11875 | 2361.620026712919 | 2299.6273291925386 | 3259.7881633716934 | 1883.3139274518542 |
| candidate (dwell 1) | 4 | 309728 | 128.13675000000032 | 3.11875 | 2359.7334968820296 | 2221.007591442366 | 3247.7859064410522 | 2001.86846792351 |

#### Standard (production CP pair)

| Controller | Seed | B delivered bits | Data air s | Switch air s | B x1 bps | Joint best fixed bps | Joint oracle bps | Arm A bps |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| baseline (dwell 5) | 0 | 350192 | 156.3469999999996 | 6.487 | 2150.6073670118085 | 2491.3623188405863 | 3618.2186707233354 | 1740.9539127271348 |
| baseline (dwell 5) | 1 | 353472 | 157.2439999999996 | 6.487 | 2158.8581270498617 | 2519.9694889397483 | 3658.3252238956748 | 1938.5379800419187 |
| baseline (dwell 5) | 2 | 344648 | 155.21999999999983 | 6.487 | 2131.311569690863 | 2498.2456140350955 | 3662.812316559057 | 1861.952793619225 |
| baseline (dwell 5) | 3 | 356576 | 146.80199999999962 | 6.487 | 2326.168218202225 | 2563.4172387490544 | 3665.815170102382 | 1854.1562734885842 |
| baseline (dwell 5) | 4 | 363328 | 139.2349999999998 | 6.487 | 2493.295452985826 | 2563.4172387490544 | 3704.09838739494 | 1813.285171152427 |
| candidate (dwell 1) | 0 | 335920 | 121.78999999999965 | 2.7445 | 2697.405136729187 | 2491.3623188405863 | 3618.2186707233354 | 2359.9910067898804 |
| candidate (dwell 1) | 1 | 339184 | 121.43199999999968 | 2.7445 | 2731.4669039633172 | 2519.9694889397483 | 3658.3252238956748 | 2383.609770130449 |
| candidate (dwell 1) | 2 | 331600 | 118.90199999999963 | 2.7445 | 2725.9312845005898 | 2498.2456140350955 | 3662.812316559057 | 2407.3926961757925 |
| candidate (dwell 1) | 3 | 344704 | 116.76299999999962 | 2.7445 | 2884.3712737694377 | 2563.4172387490544 | 3665.815170102382 | 2375.044919495507 |
| candidate (dwell 1) | 4 | 353488 | 117.89299999999962 | 2.7445 | 2930.166822090984 | 2563.4172387490544 | 3704.09838739494 | 2386.981398098883 |

C1, robust historical, seed 0: baseline arm A adaptive 330144, candidate arm A-d1 adaptive 301032, fixed 358848, oracle 429256. Candidate ratios 0.838884430177 and 0.701287809605; predicates `301032 > 358848` = False and `5 * 301032 >= 4 * 429256` = False. **C1 falls** (from 0.920010700910 / 0.769107478987): the candidate spends more frames at faster levels, which this airtime-blind slot count cannot credit. The bench's line:

```text
COP-183 legacy seed=0 arm=A-d1 adaptive=301032 fixed=358848 oracle=429256 bars=Some((false, (None = non-decisional)
```

C2, standard production: arm B-d1 x1 sums [13969.341421053516, 12636.411899313538, 18309.26976867539]. Ratios of means 1.105483228337 and 0.762965514056; >1.0 = **True**, >=0.8 = False. The bench's line:

```text
COP-183 standard arm-B-d1-x1 operands=[[2697.405136729187, 2491.3623188405863, 3618.2186707233354], [2731.4669039633172, 2519.9694889397483, 3658.3252238956748], [2725.9312845005898, 2498.2456140350955, 3662.812316559057], [2884.3712737694377, 2563.4172387490544, 3665.815170102382], [2930.166822090984, 2563.4172387490544, 3704.09838739494]] bars=Some((true, false)) (None = non-decisional)
```

Robust arm B-d1 x1 (descriptive): [11667.471917026447, 11321.242236024804, 16287.8424295188], ratios 1.030582304820 and 0.716330107411.

| Profile | Comparison | Baseline (dwell 5) | Candidate (dwell 1) |
|---|---|---|---|
| robust | C1 adaptive/best-fixed, adaptive/oracle (slot counts, seed 0) | 0.9200, 0.7691 | 0.8389, 0.7013 |
| standard | C2 arm B x1 / best-fixed, / oracle | 0.8911, 0.6150 | 1.1055, 0.7630 |
| robust | arm B x1 / best-fixed, / oracle (descriptive) | 0.7954, 0.5529 | 1.0306, 0.7163 |
| robust | arm A sum (long CP fixed), bps | 7564.584147 | 9849.245217 |
| standard | arm A sum (long CP fixed), bps | 9208.886131 | 11913.019791 |

Pooled arm B x1 goodput by Watterson segment, over all five seeds (switch air charged to the segment of the deciding frame):

| Profile | Segment | Baseline bits | Baseline data air s | Baseline bps | Candidate bits | Candidate data air s | Candidate bps |
|---|---|---:|---:|---:|---:|---:|---:|
| robust | Good | 187536 | 215.05000 | 872.0577 | 116960 | 131.69800 | 888.0925 |
| robust | Poor | 178304 | 242.21300 | 736.1455 | 148312 | 133.17000 | 1113.7043 |
| standard | Good | 186848 | 174.11000 | 1073.1606 | 138160 | 117.18500 | 1178.9905 |
| standard | Poor | 184472 | 189.88800 | 971.4779 | 149008 | 118.10500 | 1261.6570 |

No handshake was decided inside either Watterson segment on any seed, so the segment switch air is zero throughout.

coppad's default shape, replayed (long CP fixed, probing off), per-seed goodput in bps:

| Profile | Seed | Dwell 5 | Dwell 1 |
|---|---:|---:|---:|
| robust | 0 | 1252.603416340813 | 1543.9092692824047 |
| robust | 1 | 1170.341656215561 | 1607.4822748529168 |
| robust | 2 | 1243.459829059831 | 1630.8559560896008 |
| robust | 3 | 1275.120251373772 | 1650.3036027526641 |
| robust | 4 | 1275.2527661931956 | 1693.9295998119505 |
| robust | sum | 6216.777919183172 | 8126.480702789537 |
| standard | 0 | 1509.0921955706494 | 1855.9914733085134 |
| standard | 1 | 1512.8197041112528 | 2010.932092135105 |
| standard | 2 | 1450.7856196327457 | 1901.9647623364256 |
| standard | 3 | 1359.5116899142795 | 1985.099752439205 |
| standard | 4 | 1468.66454222885 | 1955.8025763496944 |
| standard | sum | 7300.873751457777 | 9709.790656568943 |

Adoption gates:

| Profile | G1 arm B x1 sum and every seed | G2 pooled Good and Poor | G3 arm A sum | G4 coppad default sum |
|---|---|---|---|---|
| robust | true: 9005.4227 to 11667.4719; every seed rises, smallest rise 331.1616 bps | true: Good 872.0577 to 888.0925, Poor 736.1455 to 1113.7043 | true: 7564.5841 to 9849.2452 | true: 6216.7779 to 8126.4807 |
| standard | true: 11260.2407 to 13969.3414; every seed rises, smallest rise 436.8714 bps | true: Good 1073.1606 to 1178.9905, Poor 971.4779 to 1261.6570 | true: 9208.8861 to 11913.0198 | true: 7300.8738 to 9709.7907 |

The bench's own lines:

```text
COP-183 gates base=robust gates=Some(AdoptionGates { g1: true, g2: true, g3: true, g4: true }) all=Some(true)
COP-183 gates base=standard gates=Some(AdoptionGates { g1: true, g2: true, g3: true, g4: true }) all=Some(true)
```

### Replay self-check

Every run replays arms A, B, A-d1 and B-d1 from the 18 fixed cells through the real `RateLoop` and `CpPolicy` and compares every `FrameSlot` and `CpSwitch`. Totals:

```text
acc-robust: COP-183 replay-check total_mismatches=0
acc-standard: COP-183 replay-check total_mismatches=0
```

So the dwell sweep and G4 are valid replay results on every run reported here.

### Verdict: adopted

All four pre-registered gates hold on both profiles, and the replay is exact. Raise dwell 1 ships: `RateLoop::default_coppa()` and coppad's `build_rate_loop` (through the new private `RATE_LOOP_RAISE_DWELL`) now use it. No config key, flag, wire field or public signature changes; drops, probing (off by default in coppad), CP gating and manual levels are unchanged.

The bar is still **not met**. Of the four COP-152 predicates, only the standard best-fixed half passes (1.1055 > 1.0). The standard oracle half (0.7630 against 0.8) and both robust historical predicates (C1) fail, and C1 is lower than main's (0.8389 / 0.7013 against 0.9200 / 0.7691) because the slot count cannot see airtime. The rate-loop limitation stays open in `docs/LIMITATIONS.md` and remains tracked as COP-183.

Files changed for the adoption: `crates/coppa-ml/src/rate_loop.rs`, `crates/coppa-ml/src/cp_gate.rs` (doc only), `crates/coppa-daemon/src/event_loop.rs`, `docs/adr/008-phase3-system-layer.md`, `CHANGELOG.md`. Arms A and B stay in the bench as the pre-COP-183 control; arms A-d1 and B-d1 are now the shipped controller.

Proof budget: planning spent 33 min screening; this phase's four full runs and session bench ran concurrently on a 6-CPU quota in about 20 min of wall time for the acceptance runs, well inside the 3.5 h cap.

### Limits of the evidence

- Simulation only. One transceiver per arm, immediate per-frame feedback, no ACK loss, no turnaround, no second station. Stop-and-wait with window 1; nothing here says what windowed or burst feedback (COP-184) does to the rate loop.
- Every frame in the Watterson segments draws an independent channel (`frame_seed`), so the per-frame oracle is non-causal there: it picks each frame's level after seeing that frame's channel. No causal controller can match it on those segments, which is the main reason adaptive/oracle stays far from 0.8.
- The CP handshake is charged at its loss-free floor (x1): no control-leg retransmissions, no half-duplex turnarounds, no failed handshakes. All three omissions favour arm B and arm B-d1 alike.
- C1 (bits per frame slot) cannot credit the shorter airtime of faster levels (COP-2). It is reported, not gated.
- The dwell sweep and G4 are replay results. They are exact only while the `COP-183 replay-check` lines print zero mismatches, and only for controllers that change the sender's `(CP mode, level)` choice. Anything that changes the PHY, the recommendation, the channel or the transceiver state carried between frames needs a real run.
- The robust short-CP profile is a bench-local synthetic twin. The robust joint comparison is descriptive.

### Future directions (screened in planning, not tried)

A 90-point replay grid on the development seeds also varied the raise step (one level, or jump to the recommendation), the failure back-off (drop after 1, 2 or 3 consecutive failures) and the drop on a lower recommendation (jump, one step, or ignore). Neither point below was tried on the acceptance seeds, and each reopens more frozen parameters than this ticket allowed.

- "Dwell 1, jump up, drop after 2 failures, one-step recommendation drop": standard arm B x1 / best-fixed 1.1261, / oracle 0.7729 on development seeds, but pooled Good goodput fell 10.1%. Reopens the raise step, the failure back-off and the recommendation drop.
- "Dwell 1, jump up, one-step recommendation drop": 1.1197 / 0.7685 with no segment loss. Needs new public `RateLoop` API and reopens the raise step and the recommendation drop.

No point in the grid reached adaptive/oracle 0.8 on the standard arm B; the maximum was 0.7729.

### Complete outputs

<details><summary>Acceptance seeds, robust</summary>

```text
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_robust <-> synthetic robust short-CP (metric question, D1c)
frames/run   : 300   seeds: [0, 1, 2, 3, 4]   runs: 120
NOTE         : the short-CP half of this pair is a BENCH-LOCAL SYNTHETIC profile with an
unallocated bandwidth_id (5). `select_ofdm_profile` can never select it and the
engine can never negotiate it, so this base measures the CP lever's MAGNITUDE,
not a shippable feature. Use the `standard` base for the shippable claim.


=== PRIMARY: airtime-normalized goodput (bits/s) ===
The `airtime(s)` and `goodput` columns are DATA-AIRTIME-ONLY (= switch cost x0) for EVERY
arm, arm B included. Arm B's CP-handshake air is real and is charged separately in the
SWITCH ACCOUNTING block below (and as the x1 half of every ratio in THE BAR); it is NOT in
this table's denominator. Arms A / P / B0 / C / C' and all 18 comparator cells have no
handshake air at all, so for them x0 is the whole story.
seed   arm       deliv       bits  airtime(s)    goodput     FER   delivery 95% CI
0      A      264/300      330144      222.34     1484.9   12.0%   0.838-0.912  
0      P      264/300      330144      222.34     1484.9   12.0%   0.838-0.912  
0      B      255/300      324096      176.56     1835.7   15.0%   0.805-0.886  
0      B0     255/300      324096      175.91     1842.3   15.0%   0.805-0.886  
0      C      255/300      324096      175.12     1850.7   15.0%   0.805-0.886  
0      C'     255/300      324096      175.12     1850.7   15.0%   0.805-0.886  
0      C(fix)  228/300      324672      144.90     2240.7   24.0%   0.709-0.805    (L7)
1      A      261/300      325680      214.86     1515.8   13.0%   0.827-0.903  
1      P      261/300      325680      214.86     1515.8   13.0%   0.827-0.903  
1      B      258/300      341520      163.97     2082.8   14.0%   0.816-0.895  
1      B0     258/300      341520      163.33     2090.9   14.0%   0.816-0.895  
1      C      258/300      341520      162.54     2101.1   14.0%   0.816-0.895  
1      C'     257/300      340096      160.24     2122.4   14.3%   0.812-0.892  
1      C(fix)  227/300      323248      144.90     2230.8   24.3%   0.705-0.802    (L7)
2      A      252/300      318512      209.61     1519.6   16.0%   0.794-0.877  
2      P      252/300      318512      209.61     1519.6   16.0%   0.794-0.877  
2      B      258/300      323128      190.79     1693.6   14.0%   0.816-0.895  
2      B0     258/300      323128      190.15     1699.3   14.0%   0.816-0.895  
2      C      258/300      323128      189.36     1706.4   14.0%   0.816-0.895  
2      C'     260/300      333128      175.49     1898.3   13.3%   0.824-0.901  
2      C(fix)  237/300      337488      144.90     2329.1   21.0%   0.740-0.832    (L7)
3      A      260/300      325208      211.71     1536.1   13.3%   0.824-0.901  
3      P      260/300      325208      211.71     1536.1   13.3%   0.824-0.901  
3      B      263/300      334648      173.22     1931.9   12.3%   0.835-0.909  
3      B0     263/300      334648      172.58     1939.1   12.3%   0.835-0.909  
3      C      263/300      334648      171.79     1948.0   12.3%   0.835-0.909  
3      C'     258/300      325096      173.83     1870.2   14.0%   0.816-0.895  
3      C(fix)  234/300      333216      144.90     2299.6   22.0%   0.730-0.823    (L7)
4      A      261/300      323888      214.75     1508.2   13.0%   0.827-0.903  
4      P      261/300      323888      214.75     1508.2   13.0%   0.827-0.903  
4      B      258/300      329680      178.03     1851.9   14.0%   0.816-0.895  
4      B0     258/300      329680      177.39     1858.5   14.0%   0.816-0.895  
4      C      258/300      329680      176.59     1866.9   14.0%   0.816-0.895  
4      C'     258/300      330000      176.59     1868.7   14.0%   0.816-0.895  
4      C(fix)  226/300      321824      144.90     2221.0   24.7%   0.702-0.799    (L7)

=== COMPARATORS: 9-cell long-CP-only vs 18-cell joint level x CP (D4) ===
seed    bestfix(long)    lvl bestfix(joint)   cell  oracle(long) oracle(joint)
0              1946.0     L7         2240.7 shortL7        2732.1        3258.1
1              1966.2     L8         2230.8 shortL7        2753.7        3218.3
2              1911.6     L7         2329.1 shortL7        2708.8        3303.8
3              1971.9     L7         2299.6 shortL7        2749.7        3259.8
4              1946.0     L7         2221.0 shortL7        2750.0        3247.8

=== THE BAR (adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8) ===
Each ratio is printed at BOTH switch-cost multipliers: x0 (arm B's data airtime only) and
x1 (the loss-free nominal, one handshake charged per switch). x0 alone is what these rows
used to report, unlabelled -- an arm B that paid nothing for its own switches.
seed          B/bf(long)       B/orc(long)       B/bf(joint)      B/orc(joint)
0            0.943/0.904       0.672/0.644       0.819/0.785       0.563/0.540
1            1.059/1.012       0.756/0.723       0.934/0.892       0.647/0.618
2            0.886/0.852       0.625/0.601       0.727/0.699       0.513/0.493
3            0.980/0.939       0.703/0.673       0.840/0.805       0.593/0.568
4            0.952/0.913       0.673/0.646       0.834/0.800       0.570/0.547
(each cell is x0/x1)

REMINDER (Key Discovery 1): a uniform CP change is ONE CONSTANT airtime divisor, so it
cancels out of any ratio. The `(long)` columns let a short-CP arm bank a spurious
1260/1104 = +14.13% against a comparator DENIED the same lever; the `(joint)` columns are
the honest ones (D4). A `(joint)` miss is NOT evidence against short CP -- the bar is
structurally blind to an airtime lever.
COP-152 seed=0 arm-B bits=324096 data_air=176.55524999999974511 switch_air=7.60975000000000001
COP-152 segment seed=0 segment=2 bits=37440 data_air=37.46699999999998454 switch_air=-0.00000000000000000
COP-152 segment seed=0 segment=3 bits=30704 data_air=54.00399999999999068 switch_air=-0.00000000000000000
COP-152 seed=1 arm-B bits=341520 data_air=163.97425000000015416 switch_air=7.60975000000000001
COP-152 segment seed=1 segment=2 bits=40696 data_air=43.07899999999997931 switch_air=-0.00000000000000000
COP-152 segment seed=1 segment=3 bits=41848 data_air=35.92599999999997351 switch_air=-0.00000000000000000
COP-152 seed=2 arm-B bits=323128 data_air=190.79225000000002410 switch_air=7.60975000000000001
COP-152 segment seed=2 segment=2 bits=30080 data_air=48.48399999999996624 switch_air=-0.00000000000000000
COP-152 segment seed=2 segment=3 bits=30280 data_air=57.10899999999993781 switch_air=-0.00000000000000000
COP-152 seed=3 arm-B bits=334648 data_air=173.22025000000013506 switch_air=7.60975000000000001
COP-152 segment seed=3 segment=2 bits=41432 data_air=40.66399999999998727 switch_air=-0.00000000000000000
COP-152 segment seed=3 segment=3 bits=36464 data_air=47.58699999999996066 switch_air=-0.00000000000000000
COP-152 seed=4 arm-B bits=329680 data_air=178.02724999999986721 switch_air=7.60975000000000001
COP-152 segment seed=4 segment=2 bits=37888 data_air=45.35599999999998033 switch_air=-0.00000000000000000
COP-152 segment seed=4 segment=3 bits=39008 data_air=47.58699999999996066 switch_air=-0.00000000000000000

=== LEGACY CONTROL (airtime-blind bits-per-slot; proves the metric swap changed accounting only) ===
These are the PRE-COP-2 quantities, recomputed. On the `robust` base at seed 0 they must
reproduce the record: adaptive 326680 / best fixed (L4) 357424 / oracle 428936 /
adaptive-oracle 0.762 / adaptive-best-fixed 0.914.
COP-152 legacy seed=0 adaptive=330144 fixed=358848 oracle=429256 bars=Some((false, false)) (None = non-decisional)
seed 0   adaptive  330144 bits | best fixed (L4 )  358848 bits | oracle  429256 bits | a/o = 0.769  a/bf = 0.920
COP-152 legacy seed=1 adaptive=325680 fixed=350304 oracle=427056 bars=None (None = non-decisional)
seed 1   adaptive  325680 bits | best fixed (L4 )  350304 bits | oracle  427056 bits | a/o = 0.763  a/bf = 0.930
COP-152 legacy seed=2 adaptive=318512 fixed=367392 oracle=429416 bars=None (None = non-decisional)
seed 2   adaptive  318512 bits | best fixed (L4 )  367392 bits | oracle  429416 bits | a/o = 0.742  a/bf = 0.867
COP-152 legacy seed=3 adaptive=325208 fixed=350304 oracle=427552 bars=None (None = non-decisional)
seed 3   adaptive  325208 bits | best fixed (L4 )  350304 bits | oracle  427552 bits | a/o = 0.761  a/bf = 0.928
COP-152 legacy seed=4 adaptive=323888 fixed=364544 oracle=430880 bars=None (None = non-decisional)
seed 4   adaptive  323888 bits | best fixed (L4 )  364544 bits | oracle  430880 bits | a/o = 0.752  a/bf = 0.888
NOTE: `best fixed` here is an argmax over BITS (the legacy quantity). The primary table's
best-fixed is an argmax over GOODPUT, and the two genuinely select different levels -- that
is the metric change working, not a regression.

=== SWITCH ACCOUNTING (arm B) ===
Charged per switch: 3 frames under the old profile (Propose, Confirm, bare ack) + 2 under
the new one (CpSwitched, its ack) = the 5 droppable on-air frames cp_negotiator counts.
seed    switches   frames   switch_air %of arm B       gp x0       gp x1       gp x6
0              1        5         7.61     4.31%      1835.7      1759.8      1458.5
         switch LongCp -> ShortCp decided@3 effective@9 L1
1              1        5         7.61     4.64%      2082.8      1990.4      1629.1
         switch LongCp -> ShortCp decided@3 effective@9 L1
2              1        5         7.61     3.99%      1693.6      1628.7      1366.6
         switch LongCp -> ShortCp decided@3 effective@9 L1
3              1        5         7.61     4.39%      1931.9      1850.6      1528.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
4              1        5         7.61     4.27%      1851.9      1775.9      1473.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
x0 = no-charge upper bound, x1 = loss-free nominal, x6 = ARQ-budget ceiling
(DEFAULT_MAX_RETRANSMIT = 5). Not charged at all, all biasing arm B optimistic:
leg retransmissions, 4 half-duplex turnarounds at DEFAULT_TURNAROUND = 150 ms, and
handshakes that fail and revert via G1-G4 having spent the full budget for zero CP change.

=== CpGate DELAY-SPREAD HISTOGRAM (arm B, per schedule segment) ===
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s0 awgn ramp down      88       0       0       0       0       0       0       0        12         0
s0 watterson good      20      12       0       0       0       0       0       0        18         0
s0 watterson poor      24       0       0       0       0      14       0       0        12         0
s1 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s1 awgn ramp down      91       0       0       0       0       0       0       0         9         0
s1 watterson good      27       9       0       0       0       0       0       0        14         0
s1 watterson poor      21       1       0       0       1      13       0       0        14         0
s2 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s2 awgn ramp down      92       0       0       0       0       0       0       0         8         0
s2 watterson good      24       9       0       0       0       0       0       0        17         0
s2 watterson poor      19       0       0       0       0      17       0       0        14         0
s3 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s3 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s3 watterson good      32       6       0       0       0       0       0       0        12         0
s3 watterson poor      23       0       0       0       0      17       0       0        10         0
s4 awgn ramp up        93       0       0       0       0       0       0       0         7         0
s4 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s4 watterson good      29       8       0       0       0       0       0       0        13         0
s4 watterson poor      21       0       0       0       0      17       0       0        12         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)
COP-152 standard arm-B-x1 operands=[[1759.8132109792873, 2240.662525879909, 3258.1447162773043], [1990.3953748601252, 2230.8350586611377, 3218.2865690479243], [1628.652936966361, 2329.109730848853, 3303.837074380825], [1850.6221312835246, 2299.6273291925386, 3259.7881633716934], [1775.9390638719665, 2221.007591442366, 3247.7859064410522]] bars=None (None = non-decisional)

=== TICKET COP-2 DELTA ===
Both answers below were PRE-COMMITTED in the plan (D4a) before this run.

PRIMARY -- the denominator delta (where a uniform airtime saving actually appears):
  best-fixed:  long-CP-only    1948.3 bps  ->  joint    2264.2 bps   (+16.21%)
  oracle    :  long-CP-only    2738.9 bps  ->  joint    3257.6 bps   (+18.94%)
  (the uniform CP factor is 1260/1104 = +14.130%; a value at or near it means the lever is
working exactly as arithmetic predicts, NOT that the bar moved)

SECONDARY -- does CP *adaptivity* add anything over fixed short CP?
  arm B (CP-adaptive)    1879.2 bps   vs   arm C (adaptive rate, fixed short CP)    1894.6 bps  (-0.82%)   [x0]
  arm B (CP-adaptive)    1801.1 bps   vs   arm C (adaptive rate, fixed short CP)    1894.6 bps  (-4.94%)   [x1]
  arm B / best-fixed(joint) = 0.830 (x0) / 0.795 (x1)  (bar: > 1.0)
  arm B / oracle(joint)     = 0.577 (x0) / 0.553 (x1)  (bar: >= 0.8)
  x0 = arm B's data airtime only; x1 = its handshake air charged once per switch. Arm C
has no handshake air at all, so only arm B's side of each comparison moves.
  upper bound (NOT a CP-isolation control): best hindsight-picked FIXED short-CP cell
     2264.2 bps -- fixed rate AND fixed CP, so B/this delta still conflates both adaptive
  dimensions; kept for reference only, per-frame in the seed table above as row "C".

HEADER-CODEWORD ISOLATION (arm C vs arm C', same short CP, header bandwidth ID only):
  arm C (header id 5)    1894.6 bps   vs   arm C' (header id 3)    1922.0 bps  (+1.45%)
  for scale: B-vs-C was -0.82% (x0). If |C-vs-C'| is comparable to |B-vs-C|, the
CP-specific conclusion above is confounded by the header codeword and should not be
drawn without controlling for it.

D9 DECISION RULE -- per-seed paired deltas, decided on SIGN CONSISTENCY:
Deltas are shown at x0 and x1. The sign counts below are tallied at BOTH, because a sign
that survives x0 but not x1 is a result the no-charge accounting manufactured.
seed         B-A (bps) x0/x1       B-C (bps) x0/x1       B/bf(joint)
0              +350.8/+274.9           -15.0/-90.9       0.819/0.785
1              +567.0/+474.6          -18.4/-110.7       0.934/0.892
2              +174.0/+109.1           -12.8/-77.8       0.727/0.699
3              +395.8/+314.5           -16.1/-97.4       0.840/0.805
4              +343.7/+267.7           -15.0/-90.9       0.834/0.800
  x0: B > A on 5/5 seeds; B > C on 0/5 seeds.
  x1: B > A on 5/5 seeds; B > C on 0/5 seeds.
  Sign-consistent (all 5 seeds one way) => a DIRECTIONAL result. Mixed signs => no result,
and the write-up must say the delta is inside this bench's own run-to-run band rather than
reporting the mean as if it were an effect. Pairing is exact only at the CHANNEL level: once
delivery outcomes diverge, RateLoop's trajectory diverges too, so the arms do not transmit
the same levels on the same frames.

REBUILD PLACEBO -- arm A 1512.9 bps vs arm P 1512.9 bps (+0.00%)
  If these differ by more than this bench's run-to-run band, arm B must be read against P,
not A, and the write-up must say so. Do NOT assume the transceiver rebuild is inert.

D1a SHORT-CP AIRTIME SHARE (arm B, on the control arm's pre-reduction basis):
  seed 0   levels<=4  45.29%   levels>=5  48.92%   (total short-CP  94.21%)
  seed 1   levels<=4  41.83%   levels>=5  51.93%   (total short-CP  93.76%)
  seed 2   levels<=4  53.24%   levels>=5  41.41%   (total short-CP  94.64%)
  seed 3   levels<=4  46.91%   levels>=5  47.18%   (total short-CP  94.10%)
  seed 4   levels<=4  47.21%   levels>=5  47.04%   (total short-CP  94.26%)
  Only the levels<=4 share is production-reachable: `select_ofdm_profile` routes every level
>= 5 to `vhf_wide()` and ignores cp_mode there, so the levels>=5 share is an explicit
UPPER-BOUND OVERSHOOT this bench collects and production cannot (D1a).

TRANSITION-COUNT GATE (pre-committed): if arm B yields <= 1 CpGate transition, this
schedule does not exercise CP *adaptivity* at all and the only content is the arm B vs
arm C fixed contrast -- which short_cp_fading_gate already covers. Do not describe a
latched single switch as adaptive control.
  transitions: 5 across 5 seed(s) (mean 1.0/run)

=== COP-183 CANDIDATE: raise_dwell 1 (probing (2,1), CP gate, schedule, seeds and comparators unchanged) ===
Arms A and B above are the baseline controller (raise dwell 5, probing (2,1)). Arms A-d1 and
B-d1 are the same shapes with raise dwell 1 and nothing else changed. Each seed ran 26 arms
(the 18 cells, A, C, C', B, B0, P, A-d1, B-d1); the header's `runs` count predates COP-183.
COP-183 seed=0 arm=B bits=324096 data_air=176.55524999999974511 switch_air=7.60975000000000001 x1=1759.81321097928730524
COP-183 seed=0 arm=B-d1 bits=312152 data_air=135.96350000000020941 switch_air=3.11874999999999991 x1=2244.36978838061304486
COP-183 seed=0 arm=A goodput=1484.87771968291349367
COP-183 seed=0 arm=A-d1 goodput=1968.06001618082814275
COP-183 seed=1 arm=B bits=341520 data_air=163.97425000000015416 switch_air=7.60975000000000001 x1=1990.39537486012523004
COP-183 seed=1 arm=B-d1 bits=309752 data_air=130.30550000000022237 switch_air=3.11874999999999991 x1=2321.55698832857933667
COP-183 seed=1 arm=A goodput=1515.80417139366409174
COP-183 seed=1 arm=A-d1 goodput=1958.63164752053080520
COP-183 seed=2 arm=B bits=323128 data_air=190.79225000000002410 switch_air=7.60975000000000001 x1=1628.65293696636103959
COP-183 seed=2 arm=B-d1 bits=322776 data_air=132.49050000000022465 switch_air=3.11874999999999991 x1=2380.19161672230666227
COP-183 seed=2 arm=A goodput=1519.57300891552563371
COP-183 seed=2 arm=A-d1 goodput=2037.37115745447067638
COP-183 seed=3 arm=B bits=334648 data_air=173.22025000000013506 switch_air=7.60975000000000001 x1=1850.62213128352459535
COP-183 seed=3 arm=B-d1 bits=310752 data_air=128.46550000000024738 switch_air=3.11874999999999991 x1=2361.62002671291884326
COP-183 seed=3 arm=A goodput=1536.12847991025091687
COP-183 seed=3 arm=A-d1 goodput=1883.31392745185416970
COP-183 seed=4 arm=B bits=329680 data_air=178.02724999999986721 switch_air=7.60975000000000001 x1=1775.93906387196648211
COP-183 seed=4 arm=B-d1 bits=309728 data_air=128.13675000000031901 switch_air=3.11874999999999991 x1=2359.73349688202961261
COP-183 seed=4 arm=A goodput=1508.20076716666267203
COP-183 seed=4 arm=A-d1 goodput=2001.86846792351002478
COP-183 standard arm-B-d1-x1 operands=[[2244.369788380613, 2240.662525879909, 3258.1447162773043], [2321.5569883285793, 2230.8350586611377, 3218.2865690479243], [2380.1916167223067, 2329.109730848853, 3303.837074380825], [2361.620026712919, 2299.6273291925386, 3259.7881633716934], [2359.7334968820296, 2221.007591442366, 3247.7859064410522]] bars=None (None = non-decisional)
COP-183 legacy seed=0 arm=A-d1 adaptive=301032 fixed=358848 oracle=429256 bars=Some((false, false)) (None = non-decisional)
COP-183 legacy seed=1 arm=A-d1 adaptive=308176 fixed=350304 oracle=427056 bars=None (None = non-decisional)
COP-183 legacy seed=2 arm=A-d1 adaptive=308104 fixed=367392 oracle=429416 bars=None (None = non-decisional)
COP-183 legacy seed=3 arm=A-d1 adaptive=315408 fixed=350304 oracle=427552 bars=None (None = non-decisional)
COP-183 legacy seed=4 arm=A-d1 adaptive=297848 fixed=364544 oracle=430880 bars=None (None = non-decisional)
COP-183 segment arm=B segment=2 bits=187536 data_air=215.04999999999989768 switch_air=0.00000000000000000 pooled_x1=872.05766100906805605
COP-183 segment arm=B segment=3 bits=178304 data_air=242.21299999999982333 switch_air=0.00000000000000000 pooled_x1=736.14545874911800638
COP-183 segment arm=B-d1 segment=2 bits=116960 data_air=131.69800000000000750 switch_air=0.00000000000000000 pooled_x1=888.09245394766810477
COP-183 segment arm=B-d1 segment=3 bits=148312 data_air=133.17000000000001592 switch_air=0.00000000000000000 pooled_x1=1113.70428775249661157

=== COP-183 REPLAY SELF-CHECK ===
Each adaptive arm replayed from the 18 fixed cells with `coppa_bench::rate_replay::replay`
(the real RateLoop and CpPolicy). Zero mismatches is what licenses the replay-only numbers
below (the dwell sweep and the coppad-default guard).
COP-183 replay-check seed=0 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=0 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=0 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=0 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check total_mismatches=0

=== COP-183 DWELL SWEEP (replay; probing (2,1) and off) ===
Ratios of five-seed sums. B = arm B shape (CP-adaptive, latency 5) at x1 against
the joint comparators; A = arm A shape (long CP fixed) against the long-CP comparators;
legacy = arm A shape's delivered bits against the long-CP best-fixed and oracle bits.
COP-183 sweep probe=2,1 dwell=1 armB_x1_sum=11667.47191702644704492 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=1.0306 B/orc=0.7163 armA_sum=9849.24521653119336406 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=1.0110 A/orc=0.7192 legacy_bits=[1530568,1791392,2144160]
COP-183 sweep probe=2,1 dwell=2 armB_x1_sum=9673.43685235357770580 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.8545 B/orc=0.5939 armA_sum=8279.81603849251951033 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.8499 A/orc=0.6046 legacy_bits=[1619664,1791392,2144160]
COP-183 sweep probe=2,1 dwell=3 armB_x1_sum=9982.84250242783673457 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.8818 B/orc=0.6129 armA_sum=8170.26673663214751286 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.8387 A/orc=0.5966 legacy_bits=[1621200,1791392,2144160]
COP-183 sweep probe=2,1 dwell=4 armB_x1_sum=8588.14083412104810122 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.7586 B/orc=0.5273 armA_sum=7470.11249702467284806 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.7668 A/orc=0.5455 legacy_bits=[1623528,1791392,2144160]
COP-183 sweep probe=2,1 dwell=5 armB_x1_sum=9005.42271796126442496 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.7954 B/orc=0.5529 armA_sum=7564.58414706901658064 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.7765 A/orc=0.5524 legacy_bits=[1623432,1791392,2144160]
COP-183 sweep probe=2,1 dwell=6 armB_x1_sum=7427.88399140273577359 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.6561 B/orc=0.4560 armA_sum=6916.58955442479236808 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.7100 A/orc=0.5051 legacy_bits=[1601368,1791392,2144160]
COP-183 sweep probe=2,1 dwell=8 armB_x1_sum=6950.34978429375041742 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.6139 B/orc=0.4267 armA_sum=6476.83664555536051921 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.6649 A/orc=0.4730 legacy_bits=[1576640,1791392,2144160]
COP-183 sweep probe=off dwell=1 armB_x1_sum=10126.97669367610797053 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.8945 B/orc=0.6218 armA_sum=8126.48070278953673551 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.8342 A/orc=0.5934 legacy_bits=[1612520,1791392,2144160]
COP-183 sweep probe=off dwell=2 armB_x1_sum=8420.47514973645593273 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.7438 B/orc=0.5170 armA_sum=7152.98063378778533661 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.7343 A/orc=0.5223 legacy_bits=[1621856,1791392,2144160]
COP-183 sweep probe=off dwell=3 armB_x1_sum=7778.92651885501618381 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.6871 B/orc=0.4776 armA_sum=6836.59094001532685070 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.7018 A/orc=0.4992 legacy_bits=[1615016,1791392,2144160]
COP-183 sweep probe=off dwell=4 armB_x1_sum=7351.26972205524816673 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.6493 B/orc=0.4513 armA_sum=6501.23142433471366530 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.6674 A/orc=0.4747 legacy_bits=[1599976,1791392,2144160]
COP-183 sweep probe=off dwell=5 armB_x1_sum=6913.33059970161411911 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.6107 B/orc=0.4244 armA_sum=6216.77791918317234376 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.6382 A/orc=0.4540 legacy_bits=[1577584,1791392,2144160]
COP-183 sweep probe=off dwell=6 armB_x1_sum=6654.24360045283083309 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.5878 B/orc=0.4085 armA_sum=6018.77824548263197357 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.6178 A/orc=0.4395 legacy_bits=[1567016,1791392,2144160]
COP-183 sweep probe=off dwell=8 armB_x1_sum=6206.77187045801201748 bf_joint_sum=11321.24223602480560658 orc_joint_sum=16287.84242951879787142 B/bf=0.5482 B/orc=0.3811 armA_sum=5682.07361630090417748 bf_long_sum=9741.71428571426076815 orc_long_sum=13694.36502466952333634 A/bf=0.5833 A/orc=0.4149 legacy_bits=[1528088,1791392,2144160]

=== COP-183 COPPAD-DEFAULT GUARD (replay: long CP fixed, probing off) ===
COP-183 coppad-default seed=0 dwell5=1252.60341634081305529 dwell1=1543.90926928240469351
COP-183 coppad-default seed=1 dwell5=1170.34165621556098813 dwell1=1607.48227485291681660
COP-183 coppad-default seed=2 dwell5=1243.45982905983100864 dwell1=1630.85595608960079517
COP-183 coppad-default seed=3 dwell5=1275.12025137377190731 dwell1=1650.30360275266411918
COP-183 coppad-default seed=4 dwell5=1275.25276619319561178 dwell1=1693.92959981195053842
COP-183 coppad-default sum dwell5=6216.77791918317234376 dwell1=8126.48070278953673551

=== COP-183 ADOPTION GATES ===
Pre-registered (docs/benchmarks/ COP-183 log): G1 arm B-d1 x1 beats arm B x1 in the five-seed
sum and on every seed; G2 pooled Watterson Good and Poor x1 goodput do not fall; G3 arm A-d1's
sum does not fall below arm A's; G4 the replayed coppad default does not fall. All four, on
both profiles, adopt raise dwell 1. None = non-decisional (not the five-seed 300-frame run).
COP-183 gates base=robust gates=Some(AdoptionGates { g1: true, g2: true, g3: true, g4: true }) all=Some(true)

 frame   snr(dB)  level   (arm B, seed 0; level tracks the channel)
     0      3.0     L1
    15      7.1     L5
    30     11.1     L8
    45     15.1     L8
    60     19.2     L9
    75     23.2     L9
    90     27.3     L9
   105     28.6     L8
   120     24.6     L9
   135     20.5     L9
   150     16.5     L9
   165     12.5     L8
   180      8.4     L8
   195      4.4     L8
   210     24.0     L6
   225     24.0     L2
   240     24.0     L4
   255     24.0     L2
   270     24.0     L1
   285     24.0     L3
```

</details>

<details><summary>Acceptance seeds, standard</summary>

```text
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_standard <-> hf_standard_short_cp (production-faithful, D1)
frames/run   : 300   seeds: [0, 1, 2, 3, 4]   runs: 120


=== PRIMARY: airtime-normalized goodput (bits/s) ===
The `airtime(s)` and `goodput` columns are DATA-AIRTIME-ONLY (= switch cost x0) for EVERY
arm, arm B included. Arm B's CP-handshake air is real and is charged separately in the
SWITCH ACCOUNTING block below (and as the x1 half of every ratio in THE BAR); it is NOT in
this table's denominator. Arms A / P / B0 / C / C' and all 18 comparator cells have no
handshake air at all, so for them x0 is the whole story.
seed   arm       deliv       bits  airtime(s)    goodput     FER   delivery 95% CI
0      A      276/300      340968      195.85     1741.0    8.0%   0.884-0.946  
0      P      276/300      340968      195.85     1741.0    8.0%   0.884-0.946  
0      B      272/300      350192      156.35     2239.8    9.3%   0.868-0.935  
0      B0     272/300      350192      155.79     2247.9    9.3%   0.868-0.935  
0      C      272/300      350192      155.11     2257.7    9.3%   0.868-0.935  
0      C'     272/300      351632      155.66     2258.9    9.3%   0.868-0.935  
0      C(fix)  204/300      257856      103.50     2491.4   32.0%   0.625-0.730    (L8)
1      A      268/300      349184      180.13     1938.5   10.7%   0.853-0.923  
1      P      268/300      349184      180.13     1938.5   10.7%   0.853-0.923  
1      B      275/300      353472      157.24     2247.9    8.3%   0.880-0.943  
1      B0     275/300      353472      156.68     2255.9    8.3%   0.880-0.943  
1      C      275/300      353472      156.01     2265.7    8.3%   0.880-0.943  
1      C'     275/300      352832      156.01     2261.6    8.3%   0.880-0.943  
1      C(fix)  232/300      330368      131.10     2520.0   22.7%   0.723-0.817    (L7)
2      A      267/300      344040      184.77     1862.0   11.0%   0.850-0.921  
2      P      267/300      344040      184.77     1862.0   11.0%   0.850-0.921  
2      B      268/300      344648      155.22     2220.4   10.7%   0.853-0.923  
2      B0     268/300      344648      154.66     2228.4   10.7%   0.853-0.923  
2      C      268/300      344648      153.98     2238.2   10.7%   0.853-0.923  
2      C'     268/300      344968      153.98     2240.3   10.7%   0.853-0.923  
2      C(fix)  230/300      327520      131.10     2498.2   23.3%   0.716-0.811    (L7)
3      A      271/300      348440      187.92     1854.2    9.7%   0.865-0.932  
3      P      271/300      348440      187.92     1854.2    9.7%   0.865-0.932  
3      B      271/300      356576      146.80     2429.0    9.7%   0.865-0.932  
3      B0     271/300      356576      146.24     2438.2    9.7%   0.865-0.932  
3      C      271/300      356576      145.57     2449.6    9.7%   0.865-0.932  
3      C'     271/300      356256      145.57     2447.4    9.7%   0.865-0.932  
3      C(fix)  236/300      336064      131.10     2563.4   21.3%   0.737-0.829    (L7)
4      A      274/300      346376      191.02     1813.3    8.7%   0.876-0.940  
4      P      274/300      346376      191.02     1813.3    8.7%   0.876-0.940  
4      B      273/300      363328      139.23     2609.5    9.0%   0.872-0.937  
4      B0     273/300      363328      138.68     2620.0    9.0%   0.872-0.937  
4      C      273/300      363328      138.00     2632.8    9.0%   0.872-0.937  
4      C'     274/300      364752      141.47     2578.2    8.7%   0.876-0.940  
4      C(fix)  236/300      336064      131.10     2563.4   21.3%   0.737-0.829    (L7)

=== COMPARATORS: 9-cell long-CP-only vs 18-cell joint level x CP (D4) ===
seed    bestfix(long)    lvl bestfix(joint)   cell  oracle(long) oracle(joint)
0              2158.9     L9         2491.4 shortL8        3040.7        3618.2
1              2160.4     L7         2520.0 shortL7        3055.6        3658.3
2              2208.0     L7         2498.2 shortL7        3072.8        3662.8
3              2198.5     L7         2563.4 shortL7        3087.1        3665.8
4              2169.9     L7         2563.4 shortL7        3081.6        3704.1

=== THE BAR (adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8) ===
Each ratio is printed at BOTH switch-cost multipliers: x0 (arm B's data airtime only) and
x1 (the loss-free nominal, one handshake charged per switch). x0 alone is what these rows
used to report, unlabelled -- an arm B that paid nothing for its own switches.
seed          B/bf(long)       B/orc(long)       B/bf(joint)      B/orc(joint)
0            1.037/0.996       0.737/0.707       0.899/0.863       0.619/0.594
1            1.041/0.999       0.736/0.707       0.892/0.857       0.614/0.590
2            1.006/0.965       0.723/0.694       0.889/0.853       0.606/0.582
3            1.105/1.058       0.787/0.754       0.948/0.907       0.663/0.635
4            1.203/1.149       0.847/0.809       1.018/0.973       0.704/0.673
(each cell is x0/x1)

REMINDER (Key Discovery 1): a uniform CP change is ONE CONSTANT airtime divisor, so it
cancels out of any ratio. The `(long)` columns let a short-CP arm bank a spurious
1260/1104 = +14.13% against a comparator DENIED the same lever; the `(joint)` columns are
the honest ones (D4). A `(joint)` miss is NOT evidence against short CP -- the bar is
structurally blind to an airtime lever.
COP-152 seed=0 arm-B bits=350192 data_air=156.34699999999961051 switch_air=6.48700000000000010
COP-152 segment seed=0 segment=2 bits=38096 data_air=34.47700000000001097 switch_air=-0.00000000000000000
COP-152 segment seed=0 segment=3 bits=34112 data_air=43.69999999999999574 switch_air=-0.00000000000000000
COP-152 seed=1 arm-B bits=353472 data_air=157.24399999999960187 switch_air=6.48700000000000010
COP-152 segment seed=1 segment=2 bits=39840 data_air=32.75200000000001666 switch_air=-0.00000000000000000
COP-152 segment seed=1 segment=3 bits=33904 data_air=46.41399999999999437 switch_air=-0.00000000000000000
COP-152 seed=2 arm-B bits=344648 data_air=155.21999999999982833 switch_air=6.48700000000000010
COP-152 segment seed=2 segment=2 bits=28128 data_air=41.79099999999999682 switch_air=-0.00000000000000000
COP-152 segment seed=2 segment=3 bits=36472 data_air=34.98300000000002541 switch_air=-0.00000000000000000
COP-152 seed=3 arm-B bits=356576 data_air=146.80199999999962301 switch_air=6.48700000000000010
COP-152 segment seed=3 segment=2 bits=39560 data_air=34.73000000000001108 switch_air=-0.00000000000000000
COP-152 segment seed=3 segment=3 bits=37288 data_air=33.99400000000001398 switch_air=-0.00000000000000000
COP-152 seed=4 arm-B bits=363328 data_air=139.23499999999978627 switch_air=6.48700000000000010
COP-152 segment seed=4 segment=2 bits=41224 data_air=30.36000000000002075 switch_air=-0.00000000000000000
COP-152 segment seed=4 segment=3 bits=42696 data_air=30.79700000000002547 switch_air=-0.00000000000000000

=== LEGACY CONTROL (airtime-blind bits-per-slot; proves the metric swap changed accounting only) ===
These are the PRE-COP-2 quantities, recomputed. On the `robust` base at seed 0 they must
reproduce the record: adaptive 326680 / best fixed (L4) 357424 / oracle 428936 /
adaptive-oracle 0.762 / adaptive-best-fixed 0.914.
COP-152 legacy seed=0 adaptive=340968 fixed=371664 oracle=436712 bars=None (None = non-decisional)
seed 0   adaptive  340968 bits | best fixed (L4 )  371664 bits | oracle  436712 bits | a/o = 0.781  a/bf = 0.917
COP-152 legacy seed=1 adaptive=349184 fixed=370240 oracle=436104 bars=None (None = non-decisional)
seed 1   adaptive  349184 bits | best fixed (L4 )  370240 bits | oracle  436104 bits | a/o = 0.801  a/bf = 0.943
COP-152 legacy seed=2 adaptive=344040 fixed=364544 oracle=437400 bars=None (None = non-decisional)
seed 2   adaptive  344040 bits | best fixed (L4 )  364544 bits | oracle  437400 bits | a/o = 0.787  a/bf = 0.944
COP-152 legacy seed=3 adaptive=348440 fixed=377360 oracle=438248 bars=None (None = non-decisional)
seed 3   adaptive  348440 bits | best fixed (L4 )  377360 bits | oracle  438248 bits | a/o = 0.795  a/bf = 0.923
COP-152 legacy seed=4 adaptive=346376 fixed=370240 oracle=439312 bars=None (None = non-decisional)
seed 4   adaptive  346376 bits | best fixed (L4 )  370240 bits | oracle  439312 bits | a/o = 0.788  a/bf = 0.936
NOTE: `best fixed` here is an argmax over BITS (the legacy quantity). The primary table's
best-fixed is an argmax over GOODPUT, and the two genuinely select different levels -- that
is the metric change working, not a regression.

=== SWITCH ACCOUNTING (arm B) ===
Charged per switch: 3 frames under the old profile (Propose, Confirm, bare ack) + 2 under
the new one (CpSwitched, its ack) = the 5 droppable on-air frames cp_negotiator counts.
seed    switches   frames   switch_air %of arm B       gp x0       gp x1       gp x6
0              1        5         6.49     4.15%      2239.8      2150.6      1793.4
         switch LongCp -> ShortCp decided@3 effective@9 L1
1              1        5         6.49     4.13%      2247.9      2158.9      1801.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
2              1        5         6.49     4.18%      2220.4      2131.3      1775.2
         switch LongCp -> ShortCp decided@3 effective@9 L1
3              1        5         6.49     4.42%      2429.0      2326.2      1919.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
4              1        5         6.49     4.66%      2609.5      2493.3      2039.4
         switch LongCp -> ShortCp decided@3 effective@9 L1
x0 = no-charge upper bound, x1 = loss-free nominal, x6 = ARQ-budget ceiling
(DEFAULT_MAX_RETRANSMIT = 5). Not charged at all, all biasing arm B optimistic:
leg retransmissions, 4 half-duplex turnarounds at DEFAULT_TURNAROUND = 150 ms, and
handshakes that fail and revert via G1-G4 having spent the full budget for zero CP change.

=== CpGate DELAY-SPREAD HISTOGRAM (arm B, per schedule segment) ===
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s0 awgn ramp down      98       0       0       0       0       0       0       0         2         0
s0 watterson good      25      11       0       0       0       0       0       0        14         0
s0 watterson poor      24       0       0       0       0      14       0       0        12         0
s1 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s1 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s1 watterson good      22      15       0       0       0       0       0       0        13         0
s1 watterson poor      20       0       0       0       0      19       0       0        11         0
s2 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s2 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s2 watterson good      28       5       0       0       0       0       0       0        17         0
s2 watterson poor      15       0       0       0       0      21       0       0        14         0
s3 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s3 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s3 watterson good      32       4       0       0       0       0       0       0        14         0
s3 watterson poor      22       0       0       0       0      14       0       0        14         0
s4 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s4 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s4 watterson good      24      11       0       0       0       0       0       0        15         0
s4 watterson poor      17       0       0       0       0      22       0       0        11         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)
COP-152 standard arm-B-x1 operands=[[2150.6073670118085, 2491.3623188405863, 3618.2186707233354], [2158.8581270498617, 2519.9694889397483, 3658.3252238956748], [2131.311569690863, 2498.2456140350955, 3662.812316559057], [2326.168218202225, 2563.4172387490544, 3665.815170102382], [2493.295452985826, 2563.4172387490544, 3704.09838739494]] bars=Some((false, false)) (None = non-decisional)

=== TICKET COP-2 DELTA ===
Both answers below were PRE-COMMITTED in the plan (D4a) before this run.

PRIMARY -- the denominator delta (where a uniform airtime saving actually appears):
  best-fixed:  long-CP-only    2179.1 bps  ->  joint    2527.3 bps   (+15.98%)
  oracle    :  long-CP-only    3067.6 bps  ->  joint    3661.9 bps   (+19.37%)
  (the uniform CP factor is 1260/1104 = +14.130%; a value at or near it means the lever is
working exactly as arithmetic predicts, NOT that the bar moved)

SECONDARY -- does CP *adaptivity* add anything over fixed short CP?
  arm B (CP-adaptive)    2349.3 bps   vs   arm C (adaptive rate, fixed short CP)    2368.8 bps  (-0.82%)   [x0]
  arm B (CP-adaptive)    2252.0 bps   vs   arm C (adaptive rate, fixed short CP)    2368.8 bps  (-4.93%)   [x1]
  arm B / best-fixed(joint) = 0.930 (x0) / 0.891 (x1)  (bar: > 1.0)
  arm B / oracle(joint)     = 0.642 (x0) / 0.615 (x1)  (bar: >= 0.8)
  x0 = arm B's data airtime only; x1 = its handshake air charged once per switch. Arm C
has no handshake air at all, so only arm B's side of each comparison moves.
  upper bound (NOT a CP-isolation control): best hindsight-picked FIXED short-CP cell
     2527.3 bps -- fixed rate AND fixed CP, so B/this delta still conflates both adaptive
  dimensions; kept for reference only, per-frame in the seed table above as row "C".

HEADER-CODEWORD ISOLATION (arm C vs arm C', same short CP, header bandwidth ID only):
  arm C (header id 4)    2368.8 bps   vs   arm C' (header id 1)    2357.3 bps  (-0.49%)
  for scale: B-vs-C was -0.82% (x0). If |C-vs-C'| is comparable to |B-vs-C|, the
CP-specific conclusion above is confounded by the header codeword and should not be
drawn without controlling for it.

D9 DECISION RULE -- per-seed paired deltas, decided on SIGN CONSISTENCY:
Deltas are shown at x0 and x1. The sign counts below are tallied at BOTH, because a sign
that survives x0 but not x1 is a result the no-charge accounting manufactured.
seed         B-A (bps) x0/x1       B-C (bps) x0/x1       B/bf(joint)
0              +498.9/+409.7          -17.8/-107.1       0.899/0.863
1              +309.4/+220.3          -17.8/-106.9       0.892/0.857
2              +358.4/+269.4          -17.8/-106.9       0.889/0.853
3              +574.8/+472.0          -20.6/-123.4       0.948/0.907
4              +796.2/+680.0          -23.4/-139.5       1.018/0.973
  x0: B > A on 5/5 seeds; B > C on 0/5 seeds.
  x1: B > A on 5/5 seeds; B > C on 0/5 seeds.
  Sign-consistent (all 5 seeds one way) => a DIRECTIONAL result. Mixed signs => no result,
and the write-up must say the delta is inside this bench's own run-to-run band rather than
reporting the mean as if it were an effect. Pairing is exact only at the CHANNEL level: once
delivery outcomes diverge, RateLoop's trajectory diverges too, so the arms do not transmit
the same levels on the same frames.

REBUILD PLACEBO -- arm A 1841.8 bps vs arm P 1841.8 bps (+0.00%)
  If these differ by more than this bench's run-to-run band, arm B must be read against P,
not A, and the write-up must say so. Do NOT assume the transceiver rebuild is inert.

D1a SHORT-CP AIRTIME SHARE (arm B, on the control arm's pre-reduction basis):
  seed 0   levels<=4  45.97%   levels>=5  48.40%   (total short-CP  94.37%)
  seed 1   levels<=4  45.11%   levels>=5  49.28%   (total short-CP  94.40%)
  seed 2   levels<=4  41.79%   levels>=5  52.53%   (total short-CP  94.32%)
  seed 3   levels<=4  40.70%   levels>=5  53.29%   (total short-CP  94.00%)
  seed 4   levels<=4  33.37%   levels>=5  60.30%   (total short-CP  93.67%)
  Only the levels<=4 share is production-reachable: `select_ofdm_profile` routes every level
>= 5 to `vhf_wide()` and ignores cp_mode there, so the levels>=5 share is an explicit
UPPER-BOUND OVERSHOOT this bench collects and production cannot (D1a).

TRANSITION-COUNT GATE (pre-committed): if arm B yields <= 1 CpGate transition, this
schedule does not exercise CP *adaptivity* at all and the only content is the arm B vs
arm C fixed contrast -- which short_cp_fading_gate already covers. Do not describe a
latched single switch as adaptive control.
  transitions: 5 across 5 seed(s) (mean 1.0/run)

=== COP-183 CANDIDATE: raise_dwell 1 (probing (2,1), CP gate, schedule, seeds and comparators unchanged) ===
Arms A and B above are the baseline controller (raise dwell 5, probing (2,1)). Arms A-d1 and
B-d1 are the same shapes with raise dwell 1 and nothing else changed. Each seed ran 26 arms
(the 18 cells, A, C, C', B, B0, P, A-d1, B-d1); the header's `runs` count predates COP-183.
COP-183 seed=0 arm=B bits=350192 data_air=156.34699999999961051 switch_air=6.48700000000000010 x1=2150.60736701180849195
COP-183 seed=0 arm=B-d1 bits=335920 data_air=121.78999999999965098 switch_air=2.74449999999999994 x1=2697.40513672918677912
COP-183 seed=0 arm=A goodput=1740.95391272713482067
COP-183 seed=0 arm=A-d1 goodput=2359.99100678988043001
COP-183 seed=1 arm=B bits=353472 data_air=157.24399999999960187 switch_air=6.48700000000000010 x1=2158.85812704986165045
COP-183 seed=1 arm=B-d1 bits=339184 data_air=121.43199999999967531 switch_air=2.74449999999999994 x1=2731.46690396331723605
COP-183 seed=1 arm=A goodput=1938.53798004191867221
COP-183 seed=1 arm=A-d1 goodput=2383.60977013044885098
COP-183 seed=2 arm=B bits=344648 data_air=155.21999999999982833 switch_air=6.48700000000000010 x1=2131.31156969086305253
COP-183 seed=2 arm=B-d1 bits=331600 data_air=118.90199999999963154 switch_air=2.74449999999999994 x1=2725.93128450058975432
COP-183 seed=2 arm=A goodput=1861.95279361922507633
COP-183 seed=2 arm=A-d1 goodput=2407.39269617579248006
COP-183 seed=3 arm=B bits=356576 data_air=146.80199999999962301 switch_air=6.48700000000000010 x1=2326.16821820222503447
COP-183 seed=3 arm=B-d1 bits=344704 data_air=116.76299999999962154 switch_air=2.74449999999999994 x1=2884.37127376943772106
COP-183 seed=3 arm=A goodput=1854.15627348858424739
COP-183 seed=3 arm=A-d1 goodput=2375.04491949550720165
COP-183 seed=4 arm=B bits=363328 data_air=139.23499999999978627 switch_air=6.48700000000000010 x1=2493.29545298582615942
COP-183 seed=4 arm=B-d1 bits=353488 data_air=117.89299999999961699 switch_air=2.74449999999999994 x1=2930.16682209098416934
COP-183 seed=4 arm=A goodput=1813.28517115242698310
COP-183 seed=4 arm=A-d1 goodput=2386.98139809888289165
COP-183 standard arm-B-d1-x1 operands=[[2697.405136729187, 2491.3623188405863, 3618.2186707233354], [2731.4669039633172, 2519.9694889397483, 3658.3252238956748], [2725.9312845005898, 2498.2456140350955, 3662.812316559057], [2884.3712737694377, 2563.4172387490544, 3665.815170102382], [2930.166822090984, 2563.4172387490544, 3704.09838739494]] bars=Some((true, false)) (None = non-decisional)
COP-183 legacy seed=0 arm=A-d1 adaptive=328024 fixed=371664 oracle=436712 bars=None (None = non-decisional)
COP-183 legacy seed=1 arm=A-d1 adaptive=331432 fixed=370240 oracle=436104 bars=None (None = non-decisional)
COP-183 legacy seed=2 arm=A-d1 adaptive=331832 fixed=364544 oracle=437400 bars=None (None = non-decisional)
COP-183 legacy seed=3 arm=A-d1 adaptive=338720 fixed=377360 oracle=438248 bars=None (None = non-decisional)
COP-183 legacy seed=4 arm=A-d1 adaptive=328016 fixed=370240 oracle=439312 bars=None (None = non-decisional)
COP-183 segment arm=B segment=2 bits=186848 data_air=174.11000000000004206 switch_air=0.00000000000000000 pooled_x1=1073.16064556889296000
COP-183 segment arm=B segment=3 bits=184472 data_air=189.88800000000003365 switch_air=0.00000000000000000 pooled_x1=971.47792382878310491
COP-183 segment arm=B-d1 segment=2 bits=138160 data_air=117.18500000000003070 switch_air=0.00000000000000000 pooled_x1=1178.99048513034904317
COP-183 segment arm=B-d1 segment=3 bits=149008 data_air=118.10500000000001819 switch_air=0.00000000000000000 pooled_x1=1261.65700012700540356

=== COP-183 REPLAY SELF-CHECK ===
Each adaptive arm replayed from the 18 fixed cells with `coppa_bench::rate_replay::replay`
(the real RateLoop and CpPolicy). Zero mismatches is what licenses the replay-only numbers
below (the dwell sweep and the coppad-default guard).
COP-183 replay-check seed=0 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=0 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=0 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=0 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=1 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=2 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=3 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=A slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=B slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=A-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check seed=4 arm=B-d1 slot_mismatches=0 switch_mismatches=0 first_mismatch_frame=None
COP-183 replay-check total_mismatches=0

=== COP-183 DWELL SWEEP (replay; probing (2,1) and off) ===
Ratios of five-seed sums. B = arm B shape (CP-adaptive, latency 5) at x1 against
the joint comparators; A = arm A shape (long CP fixed) against the long-CP comparators;
legacy = arm A shape's delivered bits against the long-CP best-fixed and oracle bits.
COP-183 sweep probe=2,1 dwell=1 armB_x1_sum=13969.34142105351565988 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=1.1055 B/orc=0.7630 armA_sum=11913.01979069051230908 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=1.0934 A/orc=0.7767 legacy_bits=[1658024,1854048,2187776]
COP-183 sweep probe=2,1 dwell=2 armB_x1_sum=12377.42105789242123137 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.9795 B/orc=0.6760 armA_sum=10058.74915724464335653 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.9232 A/orc=0.6558 legacy_bits=[1740472,1854048,2187776]
COP-183 sweep probe=2,1 dwell=3 armB_x1_sum=12301.65745509756379761 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.9735 B/orc=0.6719 armA_sum=10084.74976848304686428 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.9256 A/orc=0.6575 legacy_bits=[1726376,1854048,2187776]
COP-183 sweep probe=2,1 dwell=4 armB_x1_sum=10640.38855766017331916 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.8420 B/orc=0.5811 armA_sum=9097.12815191005756787 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.8349 A/orc=0.5931 legacy_bits=[1732136,1854048,2187776]
COP-183 sweep probe=2,1 dwell=5 armB_x1_sum=11260.24073494058393408 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.8911 B/orc=0.6150 armA_sum=9208.88613102929048182 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.8452 A/orc=0.6004 legacy_bits=[1729008,1854048,2187776]
COP-183 sweep probe=2,1 dwell=6 armB_x1_sum=9499.39143320585390029 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.7517 B/orc=0.5188 armA_sum=8413.35752825034796842 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.7722 A/orc=0.5485 legacy_bits=[1709904,1854048,2187776]
COP-183 sweep probe=2,1 dwell=8 armB_x1_sum=8941.86948862457575160 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.7076 B/orc=0.4884 armA_sum=7946.64984317614289466 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.7293 A/orc=0.5181 legacy_bits=[1680744,1854048,2187776]
COP-183 sweep probe=off dwell=1 armB_x1_sum=11564.17397601400261919 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.9151 B/orc=0.6316 armA_sum=9709.79065656894454150 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.8912 A/orc=0.6331 legacy_bits=[1676920,1854048,2187776]
COP-183 sweep probe=off dwell=2 armB_x1_sum=9579.15315849963371875 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.7581 B/orc=0.5232 armA_sum=8510.88428503833529248 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.7811 A/orc=0.5549 legacy_bits=[1668488,1854048,2187776]
COP-183 sweep probe=off dwell=3 armB_x1_sum=8869.95936450530280126 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.7019 B/orc=0.4845 armA_sum=7856.74552750206385099 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.7211 A/orc=0.5122 legacy_bits=[1631728,1854048,2187776]
COP-183 sweep probe=off dwell=4 armB_x1_sum=8193.92553127292012505 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.6484 B/orc=0.4475 armA_sum=7497.33464253531565191 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.6881 A/orc=0.4888 legacy_bits=[1605784,1854048,2187776]
COP-183 sweep probe=off dwell=5 armB_x1_sum=7808.97355324226646189 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.6180 B/orc=0.4265 armA_sum=7300.87375145777787111 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.6701 A/orc=0.4760 legacy_bits=[1593216,1854048,2187776]
COP-183 sweep probe=off dwell=6 armB_x1_sum=7638.95518158132290409 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.6045 B/orc=0.4172 armA_sum=6918.57223283677558356 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.6350 A/orc=0.4511 legacy_bits=[1569976,1854048,2187776]
COP-183 sweep probe=off dwell=8 armB_x1_sum=7141.49568456562610663 bf_joint_sum=12636.41189931353801512 orc_joint_sum=18309.26976867538905935 B/bf=0.5652 B/orc=0.3900 armA_sum=6451.20947786959641235 bf_long_sum=10895.65513784460563329 orc_long_sum=15337.75930746075391653 A/bf=0.5921 A/orc=0.4206 legacy_bits=[1523072,1854048,2187776]

=== COP-183 COPPAD-DEFAULT GUARD (replay: long CP fixed, probing off) ===
COP-183 coppad-default seed=0 dwell5=1509.09219557064943729 dwell1=1855.99147330851337756
COP-183 coppad-default seed=1 dwell5=1512.81970411125280407 dwell1=2010.93209213510499467
COP-183 coppad-default seed=2 dwell5=1450.78561963274569280 dwell1=1901.96476233642556508
COP-183 coppad-default seed=3 dwell5=1359.51168991427948640 dwell1=1985.09975243920507637
COP-183 coppad-default seed=4 dwell5=1468.66454222884999581 dwell1=1955.80257634969439096
COP-183 coppad-default sum dwell5=7300.87375145777787111 dwell1=9709.79065656894454150

=== COP-183 ADOPTION GATES ===
Pre-registered (docs/benchmarks/ COP-183 log): G1 arm B-d1 x1 beats arm B x1 in the five-seed
sum and on every seed; G2 pooled Watterson Good and Poor x1 goodput do not fall; G3 arm A-d1's
sum does not fall below arm A's; G4 the replayed coppad default does not fall. All four, on
both profiles, adopt raise dwell 1. None = non-decisional (not the five-seed 300-frame run).
COP-183 gates base=standard gates=Some(AdoptionGates { g1: true, g2: true, g3: true, g4: true }) all=Some(true)

 frame   snr(dB)  level   (arm B, seed 0; level tracks the channel)
     0      3.0     L1
    15      7.1     L5
    30     11.1     L8
    45     15.1     L8
    60     19.2     L9
    75     23.2     L9
    90     27.3     L9
   105     28.6     L9
   120     24.6     L9
   135     20.5     L9
   150     16.5     L9
   165     12.5     L8
   180      8.4     L8
   195      4.4     L8
   210     24.0     L4
   225     24.0     L2
   240     24.0     L4
   255     24.0     L3
   270     24.0     L2
   285     24.0     L3
```

</details>

<details><summary>Development seeds 5-9, robust (incomplete: the run had not finished when this log was written)</summary>

```text
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_robust <-> synthetic robust short-CP (metric question, D1c)
frames/run   : 300   seeds: [5, 6, 7, 8, 9]   runs: 120
DEVELOPMENT SEEDS [5, 6, 7, 8, 9]: screening only; no predicate below is decisional
NOTE         : the short-CP half of this pair is a BENCH-LOCAL SYNTHETIC profile with an
unallocated bandwidth_id (5). `select_ofdm_profile` can never select it and the
engine can never negotiate it, so this base measures the CP lever's MAGNITUDE,
not a shippable feature. Use the `standard` base for the shippable claim.
```

</details>

<details><summary>Development seeds 5-9, standard (incomplete: the run had not finished when this log was written)</summary>

```text
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_standard <-> hf_standard_short_cp (production-faithful, D1)
frames/run   : 300   seeds: [5, 6, 7, 8, 9]   runs: 120
DEVELOPMENT SEEDS [5, 6, 7, 8, 9]: screening only; no predicate below is decisional
```

</details>

<details><summary>Session bench (incomplete: the run had not finished when this log was written)</summary>

```text
=== Session-robustness bench: 5 x 10-min ARQ sessions/preset ===
Level 2 (BPSK 1/2), window 8, SNR ramp 20->0->20 dB over the session.
Drop = ArqTx::is_failed (RTO cap reached), not a single retransmit.

  [good] trial 0: completed (602.7s, 15912 bytes, 1584.1 bytes/min)
  [good] trial 1: DROPPED (491.9s, 9126 bytes, 1113.1 bytes/min)
  [good] trial 2: completed (600.0s, 15795 bytes, 1579.4 bytes/min)
  [good] trial 3: DROPPED (449.5s, 8892 bytes, 1186.9 bytes/min)
  [good] trial 4: DROPPED (449.9s, 8307 bytes, 1107.9 bytes/min)
-> good: 2/5 sessions completed without a drop, avg net goodput 1314.3 bytes/min

  [moderate] trial 0: DROPPED (399.3s, 6903 bytes, 1037.2 bytes/min)
  [moderate] trial 1: DROPPED (403.8s, 7137 bytes, 1060.6 bytes/min)
  [moderate] trial 2: DROPPED (371.5s, 6669 bytes, 1077.0 bytes/min)
  [moderate] trial 3: DROPPED (408.8s, 7137 bytes, 1047.6 bytes/min)
  [moderate] trial 4: DROPPED (368.5s, 6552 bytes, 1066.8 bytes/min)
-> moderate: 0/5 sessions completed without a drop, avg net goodput 1057.8 bytes/min

  [poor] trial 0: DROPPED (293.3s, 1755 bytes, 359.0 bytes/min)
  [poor] trial 1: DROPPED (266.4s, 1638 bytes, 368.9 bytes/min)
  [poor] trial 2: DROPPED (246.4s, 1521 bytes, 370.4 bytes/min)
  [poor] trial 3: DROPPED (250.5s, 1404 bytes, 336.3 bytes/min)
  [poor] trial 4: DROPPED (279.8s, 1287 bytes, 276.0 bytes/min)
-> poor: 0/5 sessions completed without a drop, avg net goodput 342.1 bytes/min

=== Acceptance policy: Good deterministic regression floor ===
MET: Good completed 2/5 sessions without a drop (required at least 2/5).
Moderate and Poor are diagnostic-only; their results remain reported above.

=== COP-119 daemon-matched window-1 session arms ===
-> stop-and-wait ideal-ACK clean: 1901.0 bytes/min
  stop-and-wait ideal-ACK [good] trial 0: completed (601.9s, 12384 bytes, 1234.4 bytes/min)
  stop-and-wait ideal-ACK [good] trial 1: completed (601.7s, 7968 bytes, 794.5 bytes/min)
  stop-and-wait ideal-ACK [good] trial 2: DROPPED after 6 sends (402.4s, 6528 bytes, 973.4 bytes/min)
  stop-and-wait ideal-ACK [good] trial 3: DROPPED after 6 sends (325.6s, 6624 bytes, 1220.7 bytes/min)
  stop-and-wait ideal-ACK [good] trial 4: completed (600.7s, 13152 bytes, 1313.6 bytes/min)
-> stop-and-wait ideal-ACK good: 3/5 completed, 1107.3 bytes/min
  stop-and-wait ideal-ACK [moderate] trial 0: completed (602.5s, 10944 bytes, 1089.8 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 1: completed (601.6s, 12864 bytes, 1283.0 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 2: completed (601.5s, 11232 bytes, 1120.4 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 3: DROPPED after 6 sends (353.2s, 6336 bytes, 1076.3 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 4: completed (600.5s, 11328 bytes, 1131.8 bytes/min)
-> stop-and-wait ideal-ACK moderate: 4/5 completed, 1140.3 bytes/min
  stop-and-wait ideal-ACK [poor] trial 0: completed (600.0s, 1920 bytes, 192.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 1: completed (601.1s, 5280 bytes, 527.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 2: DROPPED after 6 sends (345.5s, 576 bytes, 100.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 3: completed (600.8s, 4896 bytes, 488.9 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 4: DROPPED after 6 sends (450.7s, 2976 bytes, 396.2 bytes/min)
-> stop-and-wait ideal-ACK poor: 3/5 completed, 340.8 bytes/min
-> stop-and-wait ACK-loss clean: 1901.0 bytes/min
  stop-and-wait ACK-loss [good] trial 0: DROPPED after 6 sends (341.5s, 5856 bytes, 1029.0 bytes/min)
  stop-and-wait ACK-loss [good] trial 1: completed (601.9s, 9024 bytes, 899.5 bytes/min)
  stop-and-wait ACK-loss [good] trial 2: completed (600.3s, 9888 bytes, 988.4 bytes/min)
  stop-and-wait ACK-loss [good] trial 3: DROPPED after 6 sends (341.6s, 5952 bytes, 1045.5 bytes/min)
  stop-and-wait ACK-loss [good] trial 4: DROPPED after 6 sends (325.8s, 6048 bytes, 1113.8 bytes/min)
-> stop-and-wait ACK-loss good: 2/5 completed, 1015.3 bytes/min
  stop-and-wait ACK-loss [moderate] trial 0: DROPPED after 6 sends (319.8s, 4896 bytes, 918.7 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 1: completed (600.0s, 8832 bytes, 883.2 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 2: DROPPED after 6 sends (324.8s, 4800 bytes, 886.6 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 3: DROPPED after 6 sends (285.8s, 5088 bytes, 1068.2 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 4: completed (600.1s, 9216 bytes, 921.5 bytes/min)
-> stop-and-wait ACK-loss moderate: 2/5 completed, 935.6 bytes/min
  stop-and-wait ACK-loss [poor] trial 0: completed (600.3s, 672 bytes, 67.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 1: DROPPED after 6 sends (451.9s, 672 bytes, 89.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 2: completed (600.3s, 672 bytes, 67.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 3: completed (600.2s, 2016 bytes, 201.5 bytes/min)
  stop-and-wait ACK-loss [poor] trial 4: DROPPED after 6 sends (175.5s, 1632 bytes, 558.1 bytes/min)
-> stop-and-wait ACK-loss poor: 3/5 completed, 196.6 bytes/min
-> rs12-6 ideal-ACK clean: 1814.5 bytes/min
  rs12-6 ideal-ACK [good] trial 0: completed (607.9s, 14675 bytes, 1448.5 bytes/min)
  rs12-6 ideal-ACK [good] trial 1: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [good] trial 2: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [good] trial 3: completed (610.9s, 14675 bytes, 1441.3 bytes/min)
  rs12-6 ideal-ACK [good] trial 4: completed (610.9s, 14675 bytes, 1441.3 bytes/min)
-> rs12-6 ideal-ACK good: 5/5 completed, 1518.5 bytes/min
  rs12-6 ideal-ACK [moderate] trial 0: completed (612.4s, 14088 bytes, 1380.4 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 1: completed (611.9s, 14088 bytes, 1381.5 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 2: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 3: completed (607.9s, 14675 bytes, 1448.5 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 4: completed (609.4s, 14675 bytes, 1444.9 bytes/min)
-> rs12-6 ideal-ACK moderate: 5/5 completed, 1457.2 bytes/min
  rs12-6 ideal-ACK [poor] trial 0: completed (610.2s, 9979 bytes, 981.1 bytes/min)
  rs12-6 ideal-ACK [poor] trial 1: completed (600.8s, 10566 bytes, 1055.2 bytes/min)
  rs12-6 ideal-ACK [poor] trial 2: completed (600.8s, 10566 bytes, 1055.2 bytes/min)
  rs12-6 ideal-ACK [poor] trial 3: completed (607.1s, 11153 bytes, 1102.3 bytes/min)
  rs12-6 ideal-ACK [poor] trial 4: completed (619.2s, 10566 bytes, 1023.9 bytes/min)
-> rs12-6 ideal-ACK poor: 5/5 completed, 1043.6 bytes/min
-> rs12-6 ACK-loss clean: 1814.5 bytes/min
```

</details>
