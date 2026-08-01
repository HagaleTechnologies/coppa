# Coppa - Claude Code Instructions

## Project Overview

Coppa is an open-source OFDM digital modem for amateur radio, written in Rust and published as a **reference implementation** of an HF modem's DSP/FEC/protocol stack. It includes a full DSP chain, a protocol stack with ARQ, an AFSK 1200/AX.25 TNC, CLI tools, a daemon, C FFI bindings, and a VARA-style TCP control interface (modeled on VARA's TCP TNC API; the modem is **not** RF/waveform-compatible with VARA and does not interoperate with it).

## Build & Test Commands

```bash
# Build
cargo build --workspace

# Fast tests (lib-only, used in CI)
cargo test --workspace --lib

# Full test suite (includes integration + proptest — run before pushing)
cargo test --workspace

# With feature flags
cargo test --workspace --features cpal-backend,websocket --lib

# Clippy (CI runs with -D warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --all -- --check

# Benchmarks
cargo bench --workspace
```

## Testing Policy

**Run `cargo test --workspace` locally before pushing for fast feedback.** The full test suite (integration tests, proptest roundtrips) now runs in CI on every push/PR (`test-full` job, Linux, `--features cpal-backend,websocket`). Local full-suite runs are still recommended before pushing — CI catches it, but local runs are faster feedback. At minimum, run `cargo test --workspace --lib` for a quick sanity check.

## Workspace Structure

12 crates under `crates/`:

| Crate | Role |
|-------|------|
| `coppa-dsp` | Pure DSP: FFT, filters, AGC, resampling |
| `coppa-codec` | Modulation: BPSK, QPSK, 8PSK, QAM, OFDM |
| `coppa-protocol` | Framing, FEC (convolutional + LDPC), ARQ, compression, sessions |
| `coppa-channel` | Channel models for testing (AWGN, fading, CFO) |
| `coppa-audio` | Audio backends: CPAL (feature-gated), WAV file I/O |
| `coppa-radio` | Radio control via rigctld CAT |
| `coppa-ml` | Adaptive link control: capacity-based MCS selection, closed-loop rate control, spread-gated short-CP, spectrum sensing |
| `coppa-engine` | Core engine: thin wrapper around CoppaTransceiver |
| `coppa-host` | VARA-style TCP control server, WebSocket JSON API |
| `coppa-ffi` | C FFI (cdylib + staticlib) with streaming decode |
| `coppa-cli` | CLI binary (`coppa`) |
| `coppa-daemon` | Daemon binary (`coppad`) |

## Key Architecture

- **CoppaTransceiver** (in `coppa-protocol`) composes CoppaModem + LDPC + constellation mappers + block interleaver. This is the main encode/decode pipeline.
- **CoppaCore** (in `coppa-engine`) is a thin ~210-line wrapper around CoppaTransceiver.
- **9 speed levels** replace old mcs_index/fec_rate/modulation config. All profiles unified at 48kHz sample rate.
- FFI uses pointer-to-pointer semantics in `coppa_engine_destroy` to prevent double-free.

## CI

Single workflow at `.github/workflows/ci.yml` runs on push/PR to main:
- `cargo check`, `cargo test --lib` (fast signal, with features), **full test suite** (`cargo test --workspace --features cpal-backend,websocket`, Linux), clippy, fmt, MSRV (1.85.0), platform checks (Linux/macOS/Windows, `--lib` only on non-Linux to save runner minutes), **cargo-deny** supply-chain check, security audit (rustsec/audit-check).

## MSRV

1.85.0 (enforced in CI and `Cargo.toml`).

## Known Limitations

- CFO (carrier frequency offset) tolerance is ±50 Hz via two-stage acquisition (coarse Moose + fine Schmidl-Cox, resolved through their ambiguity periods), not unlimited — beyond that the ambiguity resolution itself wraps, and sample-clock offset is still uncorrected
- PAPR clipping uses per-speed-level targets (6.0 dB at BPSK up to 14.0 dB at 64QAM, tuned in `SPEED_LEVELS`); the old flat/too-aggressive clipping this line used to describe was fixed well before Phase 1. **The levels 9/10 (64-QAM) LDPC-non-convergence-at-high-SNR issue this bullet used to describe is FIXED** by Task 4's NR BG2 mother code + level 10's rate-7/8→5/6 change: `tests/phase_c_loopback.rs`'s `test_snr_fer_monte_carlo` now shows FER=0.00/100 for every level (1-10) across its whole swept SNR range, confirmed by a fresh run, not carried over from an old measurement
- The Phase 2 Task 4 alpha-calibration process itself is a cautionary tale worth keeping in mind for future LDPC-parameter tuning: a normalized-min-sum scale picked from a sweep at **one** speed level (level 2) measurably improved that level's isolated FER but broke real convergence (100% frame loss, even on a clean channel) for level 10's very different operating point (highest rate, least redundancy, heaviest known-pad pinning at small payloads) — caught by `tests/phase_c_loopback.rs`'s existing tests, not by the new codec's own unit tests, which didn't happen to exercise that combination. Any future alpha/decoder-parameter change should be validated across the *whole* speed ladder (and payload-size extremes), not a single representative level, before being adopted as the shipped default
- Daemon hardware audio requires the `cpal-backend` feature; without it the daemon runs but moves no audio
- Channel adaptation is real and closed-loop, but not ML-based, and not fully calibrated. Phase 3 deleted the old EWMA channel-quality predictor and optional-model-file registry (`coppa-ml::channel_predictor`/`registry` — zero callers anywhere in the workspace, removed as dead code in Task 9) and replaced them with real, deterministic, measurement-driven controllers: `coppa_ml::RateLoop` (sender-side closed-loop speed-level control, fed a receiver-computed recommendation over the ACK), `coppa_ml::CpGate` (spread-gated short-CP recommendation), and `coppa_ml::BusyGate` (spectral-occupancy transition gate for daemon telemetry). None of these are ML/inference-based — all are deterministic functions of measurements the receiver already produces. `RateLoop`'s own acceptance bar is not yet met (see the next bullet), and `CpGate`/`BusyGate`'s threshold constants are synthetic-test-validated only, not swept against a real bench the way `SPEED_LEVEL_MIN_CAPACITY` was — both real, open items, not settled. `CpGate`'s recommendation is now wired into `coppa-daemon` for live per-frame measurement and WebSocket telemetry (`[engine] cp_gate_enabled`, off by default) — see `BENCHMARKS.md`'s short-CP coherence-time section — but this wiring is deliberately measurement/telemetry only: it does not switch the engine's CP profile automatically, since CP length (unlike speed level) isn't in-band peer-negotiable with the existing protocol. See `docs/superpowers/specs/2026-07-25-cpgate-daemon-wiring-design.md` for the full reasoning and the follow-up work (a peer-negotiation handshake) this used to leave open. **UPDATE (2026-07-29): a peer-negotiation handshake is now shipped.** `coppa_protocol::cp_negotiator` implements a receiver-initiated propose/confirm handshake gated by `cp_negotiation_enabled` (off by default, matching `cp_gate_enabled`'s convention); no peer-capability negotiation (a real, deliberate scope cut — see `docs/superpowers/specs/2026-07-29-cp-switch-peer-negotiation-design.md`'s "disjoint subsystems" finding); real daemon-to-daemon verification is via `crates/coppa-daemon/src/event_loop.rs`'s `cp_negotiation_full_handshake_converges_both_sides` test, not yet a live two-radio field test. Building this surfaced a real, serious protocol-correctness bug caught only by driving that test through the real end-to-end audio dispatch path (`decode_and_dispatch_audio`) instead of lower-level primitives: the confirming station was rebuilding its own transmitter *and* receiver onto the new CP profile before sending its bare ack, so the peer still waiting on that exact ack (its proof the Confirm was delivered, and the trigger for its own switch) was still listening on the old profile and could never decode it — a guaranteed handshake deadlock in real use, invisible to design review and to the original lower-level test. Fixed by reordering: send the ack first, while still decodable on the old profile, then switch (commit `e59bf56`). **A second, related protocol-correctness gap was found by the final whole-branch review and is shipped un-fixed, by deliberate choice, since this feature stays off by default (`cp_negotiation_enabled = false`) and poses no risk to any deployment that hasn't explicitly opted in:** that same final bare ack is itself fire-and-forget — never ARQ-tracked, never retried — and the confirmer irrevocably commits its own receiver to the new profile the instant it sends that ack. If the ack is simply lost on a real, lossy HF channel (an ordinary failure mode, not a bug), the confirmer ends up on the new CP profile while the proposer is still on the old one; the proposer's own Confirm retransmit (which *is* ARQ-tracked) is encoded under the old profile and is therefore undecodable by the confirmer's now-switched receiver, so the two stations are left permanently on mismatched, mutually-undecodable CP profiles with no automatic recovery — only a manual `Reset` (there is no production caller that sends one automatically) clears it. Closing this properly needs either a third handshake leg (the fuller, `Session`-CONNECT_REQ/ACK/CFM-style "Approach C" this feature's own design doc already considered and explicitly deferred as too ambitious for a first cut) or a bounded "revert to `LongCp` after going deaf for N seconds post-switch" fallback — neither built here. Tony was asked directly and chose to ship with this documented rather than build either fix now.
- **Phase 3's closed-loop rate adaptation (Task 4) does not meet its own acceptance bar.** `RateLoop` applies a receiver-computed speed-level recommendation (`coppa_ml::recommend_speed_level`, from the frame's own per-carrier noise variances) with hybrid raise-slow/drop-fast hysteresis, but a dedicated closed-loop bench (`crates/coppa-bench/examples/closed_loop_arq.rs`) measures adaptive/best-fixed goodput = 0.894 and adaptive/oracle = 0.751 against a required >1.0/≥0.8, confirmed via an 8-point `raise_dwell` sweep showing the shipped default is a genuine peak, not a tuning gap. Root-caused (via an ad-hoc, uncommitted diagnostic — a well-reasoned hypothesis, not yet independently verified by a committed script) to the shared `channel_capacity` metric this recommendation is built on not being invariant to which speed level a measured frame happened to use: existing calibration (`SPEED_LEVEL_MIN_CAPACITY`) was only ever swept at a fixed level-2 probe, so it never exposed a level-dependent bias that this design's own zero-extra-probe-overhead approach (recommend from whatever level the in-flight frame actually used) directly triggers — a self-reinforcing bias where climbing higher makes the next capacity reading read even higher. Not a `RateLoop` hysteresis bug; belongs to the channel-estimation/MCS-calibration layer, same accepted-shortfall pattern as Phase 2's Task 1/7 and Task 4. See `BENCHMARKS.md`'s "Phase 3 Task 4" section and `docs/adr/008-phase3-system-layer.md` (decision 5). **The level-dependence itself is now independently confirmed by a committed diagnostic** (`crates/coppa-bench/examples/capacity_metric_level_bias.rs`, real run in `BENCHMARKS.md`'s "RateLoop capacity-metric level-bias diagnosis" section), but the likely mechanism is not what the paragraph above assumed: AWGN — which has no fading/coherence-time drift — is the *one* channel where the level1→level10 mean-capacity gap clears its own noise bound (+1.581 bits/s/Hz vs. a 0.121 bound), while Good/Moderate/Poor Watterson fading all read within noise. This points away from a fading-coherence-driven bias and toward something in the per-level measurement path itself (pilot extraction, equalization, or noise-variance estimation differing systematically by modulation order) — not yet root-caused further, and out of scope for the diagnostic that found it. **This has since been root-caused, via a second committed diagnostic** (`crates/coppa-bench/examples/papr_clip_level_bias.rs`, real run in `BENCHMARKS.md`'s "RateLoop capacity-metric level-bias: root cause isolated to per-level PAPR clip targets" section): the mechanism is `SPEED_LEVELS`'s own deliberately level-dependent PAPR clip schedule (6.0 dB at BPSK up to 14.0 dB at 64-QAM, see the PAPR bullet above), which is applied to the whole frame's time-domain samples — including the pilot-bearing probe symbol and every payload OFDM symbol's pilots — before the noise-variance estimate `channel_capacity` reads is computed. Forcing every level through the same clip target (either the loosest, 14.0 dB, or the harshest, 6.0 dB) collapsed the level1→level10 AWGN capacity gap from +1.554 bits/s/Hz to −0.117/−0.061 (both within their own noise bounds), in both directions — strong, falsifiable, and confirmed evidence, not just a plausible-sounding story. **Not fixed**: the per-level clip schedule itself is an intentional, already-tuned TX tradeoff, not a bug: the actual open gap is that `coppa_ml::channel_capacity`/`recommend_speed_level` treat a measured capacity reading as level-independent "channel truth" when it in fact carries a known, deterministic, per-level self-noise floor the TX itself introduces — a future fix belongs in calibrating `SPEED_LEVEL_MIN_CAPACITY` (or the closed-loop recommender's cross-level comparison) to be clip-floor-aware, not in touching the PAPR table. Still open, and still out of `RateLoop`'s own scope to fix. **A correction was since built and
measured** (see `BENCHMARKS.md`'s "RateLoop capacity/selectivity level-bias correction" section):
`coppa_ml::recommend_speed_level` now corrects for the per-level PAPR-clip self-noise floor via a
measured `(level x SNR)` correction table, but re-measuring `closed_loop_arq` shows
adaptive/best-fixed = 0.801 and adaptive/oracle = 0.667 (previously 0.894/0.751) — *worse*, not
better, concentrated in the bench's Watterson-fading tail where the AWGN-derived correction is an
explicitly-flagged, unvalidated extrapolation. The bar remains unmet and the fix, while landed
(scoped correctly to the measurement layer, no Watterson FER regression on
`tests/phase_c_loopback.rs`), does not close this gap for this bench's schedule. **A
selectivity-gated version of this correction was designed, calibrated (200 trials/cell, AWGN +
Watterson Good/Moderate/Poor), and fully measured in a throwaway worktree — not shipped, nothing
landed on `main` from this attempt.** A weight sweep (0.0 through 1.0, bypassing the calibrated
curve) found the relationship is cleanly monotonic with no local optimum: *any* amount of the
correction hurts this bench (adaptive/best-fixed 0.886 at weight 0.0, degrading smoothly to 0.801 at
weight 1.0), so no gate curve built on top of it can beat simply not correcting — which even then
only gets close to, not confidently past, the historical 0.894/0.751 baseline. Investigating *why*
surfaced counter-evidence against this bullet's own "Not a `RateLoop` hysteresis bug" framing above:
a per-frame oracle probe shows the Watterson-fading tail's real channel realizations frequently
*do* support levels 5-10, interleaved almost frame-to-frame with only-L1/L2-survivable frames —
fast fluctuation, not a sustained bad channel — identically across every tested correction weight,
which is itself the tell that the correction table isn't the driver. `RateLoop::default_coppa()`'s
"raise slow" hysteresis (5 consecutive higher recommendations required to step up, already the
swept-optimum `raise_dwell`) very plausibly cannot accumulate 5 consecutive raises under that kind
of rapid alternation once it has dropped, staying pinned low long after the channel has partially
recovered. This does not refute that the PAPR-clip cross-level measurement bias above is real and
worth having fixed on its own terms — it's evidence that it is not the dominant cause of *this
bench's* specific fading-tail shortfall. The real next lever for this bench's acceptance bar is
therefore believed to be a hysteresis/control-policy fix for fast-fluctuating channels (e.g. faster
recovery after a spurious drop, or dwell logic sensitive to recent oscillation rate), not further
correction-table tuning. **A first such fix was designed, built, and measured (2026-07-24), and is
DISCONFIRMED — nothing shipped.** The candidate (bound the recommendation-driven drop to one step
per ack instead of jumping straight to the recommendation, matching the existing failure/timeout
drop granularity; decay the raise counter on a hold instead of hard-resetting it, so interleaved
low/high recommendations don't erase all accumulated progress) was built for real in a throwaway
worktree and re-measured against `closed_loop_arq`: 0.798/0.665 at the shipped `raise_dwell = 5`,
statistically indistinguishable from (very slightly worse than) current `main`'s 0.801/0.667. A
follow-up `raise_dwell` sweep (1 through 15) with the new logic found a monotonic relationship
favoring the most aggressive dwell tested (`dwell=1`: 0.833/0.695) rather than an interior peak —
the opposite shape from the original sweep that found 5 a genuine optimum — and even that best point
falls short of both the acceptance bar and the historical pre-PR#53 baseline (0.894/0.751). A
follow-up isolation test (reverting the drop-cliff to its original jump-straight-to-recommendation
behavior, keeping only the leaky-raise decay) produced nearly identical numbers across the same
dwell sweep, showing the bounded-drop change contributes almost nothing on its own and the
leaky-raise change alone is not an improvement at the shipped dwell value. A per-frame diagnostic
during the fading tail showed the hysteresis mechanism working exactly as designed (the loop does
reach L2 in patches it previously couldn't) but not moving the aggregate metric, because per-frame
recommendations during the tail are themselves frequently and genuinely low (not merely
under-credited by hysteresis loss), and real delivery failures recur often enough to fully reset the
raise counter via the untouched failure path regardless of the hold-decay behavior. This falsifies
the specific mechanism this fix targeted; the diagnostic evidence instead points at the raw
per-frame recommendation signal's own volatility/low readings during fast fading as the dominant
factor (plausibly connected to the still-open per-level channel-estimation/capacity-measurement
limitation elsewhere in this list, though not confirmed as the same root cause), not the shape of
`RateLoop`'s hysteresis. Full measured detail is in the local (not committed, per `.gitignore`)
design doc `docs/superpowers/specs/2026-07-24-rateloop-bounded-drop-leaky-raise-design.md`'s
"Outcome" section. **The real next lever is therefore believed to be improving the underlying
per-frame recommendation
signal's quality/stability during fast fading, not further hysteresis-mechanism tuning.** **A
follow-up diagnostic (2026-07-24, same session, no code shipped) narrowed this down further, and
ruled out the most obvious first guess.** Two quick measurements (an ad-hoc, uncommitted diagnostic
example, not a permanent bench) were run over the same `closed_loop_arq` schedule's Watterson-fading
tail (frames 200-299): (1) frame-to-frame correlation of the *oracle's* true best-achievable level
between consecutive frames is only 0.144 — the channel really is close to decorrelated frame-to-frame
here, which rules out temporal smoothing/averaging of the recommendation across recent frames as a
fix (it would mostly average in stale, already-irrelevant readings); (2) same-frame correlation
between `recommend_speed_level`'s output and the oracle's true best level for that *same* frame, when
probed via a level-1 (BPSK) transmission, is only 0.241 — a real same-frame accuracy problem, not a
timing/lag problem. A follow-up sweep of the probing level (reusing `closed_loop_arq`'s own
fixed-level-run data, no new transmissions) found this same-frame correlation rises cleanly with
probe modulation order — L1 0.241, L3 0.364, L5 0.526, L6 0.639 (all well-sampled, 38-95/100 tail
frames) — confirming `channel_capacity`/`channel_selectivity`'s documented "mode-independent because
nv is pilot-derived" design assumption does not hold at the resolution this bench needs: a
low-order-modulation frame's pilots genuinely carry less information about whether a high-order mode
would decode. But this comes bundled with why `RateLoop` can't just exploit it passively: the sample
size collapses as probe level rises (L1 95/100 -> L6 38/100 -> L9 14/100) because higher-order frames
increasingly fail to decode at all — precisely when the channel is bad, which is exactly when
`RateLoop` is pinned low and would most need the better signal. Getting a more accurate reading this
way isn't free measurement-layer tuning; it looks like it requires *active* probing (periodically
transmitting above the current level, trading airtime/decode-failure risk for signal quality) — a
materially different, larger-scoped kind of change than every fix attempted on this bench so far
(all purely receiver-side/measurement-layer, explicitly zero-extra-airtime by design, per
`closed_loop_arq.rs`'s own module doc). Not yet designed; deliberately left for a fresh cycle given
the scope jump. **This active-probing design was built (2026-07-25), de-risked, refined, and
shipped** — `RateLoop` now has opt-in `with_probing`/`level_for_next_transmission`/`on_probe_result`
(a probe is an ordinary Data frame opportunistically encoded above the current level; a failed probe
is an ordinary ARQ-retransmitted loss, no wire-format change). An exhaustive sweep of
`(probe_interval, probe_offset)`, `raise_dwell`, a rejected slow-start growing-offset variant, and a
stall-gating refinement (skip a probe while the passive signal is already climbing on its own —
adopted, small consistent win) converged on `probe_interval=2, probe_offset=1` as a genuine interior
peak (`probe_interval=1`, probing every frame, collapses to 0.471/0.565 -- forfeiting guaranteed
current-level throughput on every gamble). Measured result at the unchanged `raise_dwell=5` (left
unchanged deliberately -- `coppa-daemon`'s real production traffic still calls only `on_ack`, which
reads `raise_dwell`, and doesn't yet call the new probing API, so changing the shared default would
have silently changed production behavior for a component not using this feature):
**adaptive/best-fixed = 0.931, adaptive/oracle = 0.775** -- clearly better than both this bench's
prior state (0.801/0.667) and the historical pre-level-bias-correction baseline (0.894/0.751), but
still short of the plan's `>1.0`/`>=0.8` bar. Multiple refinements all converged on the same
~0.78/~0.93 plateau, suggesting a real ceiling for this family of designs on this bench rather than a
remaining tuning gap. Shipped anyway per the same honest-partial-progress precedent as the
capacity/selectivity level-bias correction above: real, verified, substantial improvement, gap
documented rather than hidden. Real daemon wiring (applying a probe level to one live outgoing frame
and reverting afterward) remains explicitly deferred to a future cycle. See
`crates/coppa-bench/examples/closed_loop_arq.rs`'s module doc and
`docs/superpowers/specs/2026-07-25-rateloop-active-overshoot-probing-design.md`'s "Outcome" section
for the complete sweep data. **`SPEED_LEVEL_MIN_CAPACITY` was since recalibrated from clean,
HARQ-fix-correct data (2026-07-26) and re-measured — a real, meaningful improvement on one metric,
but it does not close this bench's gap.** PR #61/#62 (see the level-9 AWGN-waterfall bullet below)
fixed a stale-IR-HARQ-accumulator bench-harness bug that had corrupted `mcs_calibration.rs`'s
per-level FER readings — the exact data `SPEED_LEVEL_MIN_CAPACITY`'s original thresholds were
calibrated from — so this table was recalibrated from a fresh, clean `mcs_calibration` run
(seed `0xCA11B`) using the same goodput-proxy argmax methodology as before. Every threshold in the
9-entry table dropped (most sharply at the top: L7 6.5→4.5, L9 7.2→6.4, L10 8.0→7.6; L4's anchor 2.6→1.8
was newly data-anchored directly at Good's lowest tested SNR, since Good
turned out to never favor anything except L4 anywhere in its tested 6-30 dB range) — full
derivation table in `BENCHMARKS.md`'s "SPEED_LEVEL_MIN_CAPACITY recalibration from clean
(post-HARQ-fix) data" section. Re-validated against the held-out seed (`mcs_compare`, `0x5A1AD`):
calibrated(C) 0.741→0.859, 2D(C,sel) 0.828→0.892 against the per-cell oracle — both improve
meaningfully, a real win on that metric. But re-measuring `closed_loop_arq` with the recalibrated
table (no other code change; `recommend_speed_level` reads the const directly) gives
**adaptive/best-fixed = 0.914, adaptive/oracle = 0.762** — both *slightly worse* than the prior
0.931/0.775, not better. The acceptance bar remains unmet, and this result is a useful negative: a
demonstrably more accurate static per-cell selector table does not transfer into a better aggregate
closed-loop result on this bench's non-stationary SNR trajectory, which is further evidence (on top
of the probe-accuracy and hysteresis-mechanism investigations above) that the remaining gap is not
in the calibration table's own accuracy. The real next lever remains believed to be the raw
per-frame recommendation signal's volatility/stability during fast fading, not further
table-calibration or hysteresis-mechanism tuning. **UPDATE (2026-07-28): the per-frame signal's
volatility itself was root-caused via a direct ground-truth comparison, and the estimator-vs-ceiling
question this line of investigation had left open is now resolved in favor of "ceiling," reinforcing
(via an independent methodology) the already-closed coarse-delay-drift investigation elsewhere in
this list rather than opening a new fixable bug.** Two diagnostics were built
(`crates/coppa-bench/examples/capacity_snr_reference_diagnosis.rs` and
`capacity_ground_truth_diagnosis.rs`) to separate "is `channel_capacity`/`channel_selectivity` a
noisy-but-fixable ESTIMATOR, or is ~0.24 same-frame correlation close to the true information
ceiling a frame-averaged metric can carry?" First, a specific and previously-untested hypothesis —
that every Watterson bench's noise convention (`awgn_seeded(&faded, snr_db, seed)`, referencing
noise power to the FADED signal's own realized power rather than a fixed clean-signal reference,
contrary to `watterson.rs`'s own module-doc warning) tautologically hides a frame's overall fade
depth from the metric — was tested directly and **disconfirmed**: a clean-referenced control
(`awgn_ref_seeded`) showed no meaningfully different behavior (self-referenced vs. clean-referenced
`corr(fade_ratio, capacity)`: -0.052 vs -0.021 on Poor, -0.268 vs -0.206 on Moderate; `corr(capacity,
selectivity)`: 0.878 vs 0.893 on Poor, 0.734 vs 0.713 on Moderate — both conventions behave alike,
and the positive capacity/selectivity correlation itself contradicts the hypothesis's predicted sign).
Second, and decisively: `channel_capacity`/`channel_selectivity` (level-1 probe, Watterson
Good/Moderate/Poor, `snr_db=24`, `TRIALS=300`, `profile=robust`) were compared directly against
GROUND TRUTH computed from the Watterson channel model's own per-tap fading gains (new
`coppa_channel::watterson::watterson_with_gains`, exposing each tap's raw complex gain array so
`H(f_k,t)` can be evaluated at the model level, calibrated against the real receiver's own
measured unit-gain noise floor, not a theoretical guess — full method in the new file's module doc)
— NOT against decode success, closing the one gap PR #57's diagnostic left open. The ground truth
signal itself was verified stable within-frame (two disjoint time-window halves: corr 0.812-1.000,
confirming the long-coherence-time assumption at the ground-truth level), yet
`corr(measured capacity, ground-truth capacity)` was -0.019 (Poor) / 0.050 (Good) / -0.224
(Moderate), and `corr(measured selectivity, ground-truth selectivity)` was 0.036 / 0.131 / -0.144 —
statistically indistinguishable from zero on every preset, including the mildest (Good, 0.5 ms
delay/0.1 Hz Doppler). The receiver's own per-carrier estimate is therefore not merely noisy but
essentially uninformative about the real, same-frame channel it measured, regardless of fade
severity — the same conclusion (genuine Rayleigh coherence time far shorter than one frame, not a
fixable per-frame estimator bug) the now-closed "coarse-delay drift Kalman tracker" investigation
reached via FER outcomes elsewhere in this list, now independently confirmed via a completely
different, decode-independent method for the capacity-metric specifically. Per that investigation's
own conclusion, this is **not re-opened** as a new measurement-layer bug to fix — the untried
candidate levers remain what that investigation already identified (coherence-time/airtime
reduction, e.g. shorter frames via `hf_standard_short_cp`, or fade-diversity interleaving), not
further estimator tuning. No RateLoop or channel-estimation code changed as a result of this
diagnosis; both new example files are diagnostics only.
- **Multi-codeword frames (Phase 3 Task 5) do not extend ACK addressing, turbo re-estimation, or persistent IR-HARQ combining to the per-codeword level.** A multi-codeword frame is retransmitted, if at all, as a whole (same `seq`, cycling RV via the existing mechanism) rather than by `(seq, codeword-index)`; turbo re-estimation and IR-HARQ's persistent cross-retransmission LLR accumulator are both scoped to `codewords <= 1` only, taking the exact pre-Task-5 decode path. These are real, deliberate scope cuts (not oversights) — see `docs/adr/007-multi-codeword-frames.md` (decisions 4-5) for the full reasoning
- **Decision 4's block-ACK cadence (Phase 3 Task 2) was never implemented.** The plan called for batching ACKs to once per received burst boundary or every 2 frames rather than one ACK per decoded frame; this sub-decision has no corresponding code (confirmed via grep across `arq.rs`/`transport.rs`) — every decoded frame still triggers its own ACK today. Does not compromise correctness (more ACKs than strictly necessary is airtime-inefficient, not incorrect), but is a real, previously-undocumented gap against the plan's literal text. See `docs/adr/008-phase3-system-layer.md` (decision 4)
- **The Phase 3 benchmark program (Task 8: `milstd`, `session`) does not clear its own acceptance targets, and both gaps are honestly measured rather than hidden.** `milstd` (`cargo run -p coppa-bench --release --example milstd`) passes 0 of 27 MIL-STD-188-110-style operating points, even with a generous +12 dB margin over the reference SNR — root-caused to the ladder's borrowed reference SNRs not transferring onto Coppa's own measured thresholds on any channel, including Good (not a fading-specific bug, though the already-tracked Watterson-Moderate/Poor channel-estimation gap below is a real, additional contributing factor for those specific rows). `session` (10-minute simulated ARQ sessions via the real `ArqTx`/`ArqRx` state machines) completes drop-free on only 3/5 Good, 0/5 Moderate, and 0/5 Poor sessions against a "zero drops on good/moderate" target — root-caused to level 2's real Good-preset FER not being zero even above its nominal threshold (~8-12% at 12 dB, ~2% by 18 dB), so a sustained low-SNR ramp trough can exhaust the ARQ's bounded retransmit budget on a non-trivial fraction of trials (not an ARQ state-machine bug). Level 9 (64-QAM 2/3) separately shows an unusually high, steep, and strongly seed-dependent AWGN SNR requirement (a real waterfall, not an SNR-independent floor, per a corrected re-measurement) that never converges at all under any tested Watterson fading up to 54 dB — worth its own future investigation. See `BENCHMARKS.md`'s "Phase 3 Task 8" section and `docs/adr/008-phase3-system-layer.md` (decision 9) for the complete, twice-corrected diagnosis. **UPDATE (2026-07-26): the AWGN-side half of this — the "steep, seed-dependent waterfall" — is FIXED, and was never a real codec/PHY limitation.** Root cause: `coppa-bench`'s `run_scenario`/`run_trial` hardcodes every synthetic trial's `seq_num` to `0` and reuses one `CoppaTransceiver` across an entire ascending SNR sweep; `CoppaTransceiver`'s IR-HARQ receive-side LLR accumulator (Phase 3 Task 3) only evicts on a successful decode, so any trial that genuinely fails at a low-SNR sweep point (expected and correct) left its accumulator un-evicted, and the next unrelated random-payload trial at the same seq got its LLRs added on top of that stale buffer — corrupting every later trial at that seq for the rest of the sweep, including ones at much higher SNR that should trivially decode. This exactly explains the reported seed-dependence (how much low-SNR contamination accumulates before the sweep reaches a genuinely-clearing SNR) and why it was invisible on levels 1-6 (their low real thresholds mean the first successful decode evicts the buffer before much poison accumulates). Fixed by unconditionally evicting each trial's HARQ buffer after every `receive()` call in `run_trial` regardless of outcome (`crates/coppa-bench/src/runner.rs`), with a regression test (`ascending_sweep_low_snr_failure_does_not_poison_later_high_snr_trials`) that reproduces the bug pre-fix. Re-measured AWGN ladder (400 trials/point, same sweep): level 9 now clears **both** FER≤10% and FER≤1% cleanly at **21.0 dB**, no residual floor (previously "18.0 dB / never, ~1-1.25% floor to 30 dB"); level 10 similarly tightens to 24.0/24.0 dB (previously 24.0/27.0 dB). **The Watterson-fading half of this bullet is NOT fixed by this and remains a real, separate, confirmed-still-open limitation**: re-measuring level 9 post-fix under both Watterson Good and Poor (200 trials/point, 6-30 dB) still shows it never clearing FER≤10% on either preset (peak goodput 330 bps Good / 132 bps Poor) — this is a genuine fading-specific gap, not a rehash of the AWGN bench bug, and the "worth its own future investigation" framing above still applies to it specifically. **UPDATE (2026-07-26, same day, follow-up task): `milstd` and `session` themselves have now been re-run against this fixed harness and re-baselined.** `milstd` (which calls `run_scenario`/`run_trial` directly and was therefore fully in-scope for the fix above) still passes 0/27 operating points at the literal reference SNR and 0/27 even at +12 dB margin — the ladder-mismatch diagnosis is unchanged — but the underlying per-cell FER numbers improved on nearly every row, most visibly at moderate/poor for levels 5-10 (e.g. level 6 poor 100%→71%, level 9 poor 100%→93%, level 10 poor 100%→97%). `session` (which manages its own `CoppaTransceiver` per session and never calls `run_trial`/`run_scenario`, so this specific fix does not apply to it directly) nonetheless also produced different numbers on re-run — Good drop-free sessions 3/5→2/5, Moderate avg goodput 363.1→1039.2 bytes/min — traced to a second, separate, already-landed fix (`4761faf`, PR #42) that changed the default Kalman payload equalizer's LLR/noise output (`TrackedTaps::equalize`, previously fed the decoder posterior tap-covariance variance instead of the intended fixed observation-noise floor) for every level's default decode path, not just `milstd`'s. Both fixes were already on `main` before this re-run; their individual contributions were not isolated (out of scope for a pure re-baseline). `session`'s acceptance verdict is unchanged (NOT MET — Good still drops on some trials, Moderate/Poor still drop on all 5). See `BENCHMARKS.md`'s "Phase 3 Task 8" section for the full fresh tables.
- `coppa-channel` models AWGN + a two-tap Watterson/ITU-R F.1487 HF channel (Rayleigh taps, Gaussian Doppler) plus an `ssb_filter` helper emulating a realistic 300-2700 Hz SSB rig audio passband. The sinusoidal `fading()` helper is AGC-test-only.
- The waveform occupies a realistic ~300-2700 Hz SSB passband (carrier offset + in-band Newman-phase preamble) with TX section leveling/bandpass conditioning and a streaming O(1) preamble sync detector (`SyncDetector`, ~0.0015-0.0035x realtime) — see `docs/adr/003-phase1-waveform-break.md`. This is a wire-format break from earlier waveform revisions; old and new are not interoperable
- The LDPC layer is a single 5G-NR-style BG2 mother code (Zc=176 lifting, `crate::fec::ldpc::NrLdpc`) shared by every speed level, rate-matched down per level via a circular buffer (`rate_match.rs`) instead of switching between nine separate per-rate 802.11 QC-LDPC codes — see `docs/adr/005-nr-bg2-ldpc.md`. Level 10's nominal code rate moved from 7/8 to **5/6** as part of this change (the audited `k_used` table). This is a wire-format break: frames encoded with the pre-this-change codec are not decodable by the new one, and vice versa, old and new are not interoperable — same pattern as the Phase 1 waveform break above. Two measured, currently-unmet gaps from that change's own acceptance targets, kept open rather than hidden: (1) the coding gain at matched rate/block-length is real but smaller than the density-evolution-based prediction (measured +0.5 dB at level 2 isolated-FEC-layer AWGN vs. a predicted ~1.8 dB; a direct layered-vs-flooding A/B on identical trials ruled out a decoder-schedule bug, so this is believed to be a real finite-length effect, not investigated further); (2) decode CPU/frame is worse than the accepted ≤3x budget across the whole ladder (3.5x-9.5x measured after a real, verified ~19% optimization of the layered update's hot loop) because the shared mother code's graph (42 base rows, Zc=176) no longer shrinks for high-rate levels the way the old per-rate codes' graphs did — closing this would need a materially larger effort (SIMD, unsafe bounds-check elision, cache-aware graph relabeling), not attempted here. See `.superpowers/sdd/p2-task-4-report.md` for the full investigation
- The frame header is soft-decoded (per-bit LLRs, full 4096-codeword Golay ML + CRC-assisted list-2, Phase 2 Task 2), a clean, verified improvement over the old hard-decision header — but the plan's original acceptance figure ("≥25 percentage points header-decode-rate gain, soft vs. hard, at 200 seeds of watterson-poor, 8 dB") was not achievable and was honestly re-derived: Task 1/7's estimation work (already merged before Task 2 started) had already made hard-decision header decoding fairly robust at 8 dB, leaving only 5-7 points of headroom. The real, reproducible gap is 6-8 percentage points at a lower SNR (~3 dB), before sync itself becomes the binding constraint
- Known-pad LLR pinning and exact max-log LLR scaling (Phase 2 Task 3) measure a real +3.0 dB gain at the FEC-isolated layer, but this gain is **invisible in a full end-to-end OFDM bench** at some operating points (e.g. `hf_standard`/level 2/short payload/AWGN) because OFDM sync, not LDPC convergence, is the sole binding constraint there — every measured frame failure at that operating point was `SyncFailed`/`HeaderCorrupt`, never `LdpcNotConverged`. Any future payload-side FEC improvement will not show up in end-to-end goodput/FER benchmarks until sync's own SNR floor is at or below the LDPC decode's floor — verify FEC-layer gains with an isolated (OFDM-bypassing) bench, not just the full pipeline
- **Watterson-fading regression on sparse-pilot HF profiles (`hf_standard`/`hf_wide`/`hf_narrow`, levels 1-4) — FIXED for levels 1-3, partially fixed for level 4.** Bisected to Phase 1 Task 5's `SyncDetector` anchoring sync timing on the first-arriving multipath tap rather than the strongest one; fixed by preferring the strongest tap unless it's more than half a cyclic prefix away from the first arrival (preserving the original anti-echo safety intent for delay spreads beyond anything this codebase's Watterson presets model). Levels 1-2 now match or exceed pre-Phase-1 Watterson-Moderate/Poor performance; level 3 is very close (within normal trial variance); level 4 (QPSK 3/4) improves substantially (peak goodput up from ~330-630 bps to ~555-1234 bps) but retains a real, smaller residual gap (72-76% of pre-Phase-1 peak goodput), not yet investigated further. See `docs/adr/004-strongest-path-timing.md`, BENCHMARKS.md's "2026-07 — Hotfix: sparse-pilot header Watterson-fading regression" section, and `.superpowers/sdd/p1-fading-regression-fix-report.md`
- **Phase 2 channel estimation (Task 1 delay-domain estimator + Task 7 Kalman tracker) — a real, unresolved regression, shipped anyway.** Replacing `LinearInterpolationEstimator` with a delay-domain ridge-LS estimator (`crate::ofdm::delay_domain::DelayDomainEstimator`) regressed Watterson-Moderate/level 2's FER≤10% threshold from 18 dB to 24 dB (needed ≥1.5 dB *better*), root-caused to the estimator's frame-global coarse-delay reference not tracking real intra-frame drift (a per-window adaptive fix was built, tested, and reverted — it improved raw channel-estimate accuracy but let occasional low-SNR windows corrupt the LDPC-facing noise variance, making full-sweep FER *worse*). A Kalman/RTS tracker (`crate::ofdm::kalman_tracker::KalmanLagSmoother`) was pulled forward specifically to close this gap; it fixed one real bug (overlapping pooled-pilot windows double-counted as independent evidence) but a systematic sweep of its AR(1) forgetting coefficient across almost two orders of magnitude showed near-total FER flatness, and the final measured threshold is **30 dB** — worse than both the pre-Phase-2 baseline and Task 1's own regressed number. Believed to be a model-class mismatch (the intra-frame drift looks more like an accumulating phase/coarse-delay reference error than stationary Rayleigh amplitude fading, which an AR(1) tap-amplitude model doesn't represent), not a tuning problem — not fixed in Phase 2. See `docs/adr/006-phase2-parametric-estimation-nr-bg2.md` (decisions 1 and 3) and `.superpowers/sdd/p2-task-1-report.md`/`p2-task-7-report.md`. **Two more attempts were built and measured (2026-07), both also shipped disabled, and the "model-class mismatch" theory above has since been falsified.** A `DriftTracker` (2-state random-walk Kalman filter tracking the coarse-delay reference per-window instead of once per frame) was combined first with fresh per-window `DelayDomainEstimator::fit` refits ("Replace," PR #41) and then with the existing AR(1) `KalmanLagSmoother` tap tracker ("Cascade," PR #42, which also fixed a real, separate bug: `TrackedTaps::equalize` was feeding the LDPC decoder Kalman posterior tap variance instead of genuine observation noise, unconditionally, applying to the default path too). Both measured Watterson-Moderate/level 2 FER≤10% at exactly **18.0 dB** — identical to each other despite different downstream mechanisms, still short of the 16.5 dB bar and a ~3 dB regression against the 15.0 dB baseline both started from. A follow-on investigation into why both landed on the same number found the theory that this is "an accumulating phase/coarse-delay reference error, not stationary Rayleigh amplitude fading" is very likely **backwards**: (1) this file's and `kalman_tracker.rs`'s own "amplitude-fading coherence time ~1-10s, much longer than one frame" assumption was never checked against `coppa-channel`'s actually-configured Watterson-Moderate Doppler spread (0.5 Hz, PSD sigma 0.25 Hz) — the channel's own verified autocorrelation formula shows a level-2 frame (1.365 s) decorrelates to ~10% by frame-end, not "much longer than a frame" at all; (2) a real-Watterson `LdpcNotConverged` failure-mode breakdown across 9-24 dB is SNR-*independent* (the signature of a hard fading-outage floor, not a fixable estimation problem), and a **flat single-tap channel** (where "stale coarse-delay reference" isn't even a coherent failure mode, since there's no multipath to have a delay reference about) at Moderate's real Doppler already produces FER comparable to or worse than the real 2-tap channel at low-to-mid SNR; (3) `DriftTracker`'s own per-window delay estimate was separately found to never converge/stabilize even at 24 dB on real Watterson-Moderate frames (unlike its clean AWGN-control convergence), because it's chasing a power-weighted average of two independently-fading taps — an inherently non-stationary quantity for a single scalar state. Net: all four attempts (Task 1, Task 7, Replace, Cascade) were very likely chasing a *symptom* of genuine Rayleigh amplitude fading — whose real coherence time is far shorter here than this codebase assumed — rather than its cause, which no phase/delay-reference correction can fix. **This closes the coarse-delay-reference line of investigation** — no further variant is planned. Untried candidate levers for whoever picks this up next: coherence-time/airtime reduction (shorter frames, e.g. the existing `hf_standard_short_cp` infrastructure) or fade-diversity interleaving, rather than further channel-estimation refinement. See `BENCHMARKS.md`'s "Coarse-delay drift Kalman tracker," "Cascaded coarse-delay drift + AR(1) tap tracker," and "Fading root-cause investigation" sections, and `crates/coppa-bench/examples/{drift_estimate_quality_diagnosis,regression_root_cause}.rs` for the measurement detail
- **Turbo re-estimation (Task 5) rescues frames on decode failure, but the benefit is heavily concentrated on low-order modulation.** One-round LDPC-aided re-estimation (soft virtual pilots from posterior LLRs, re-fit/re-demap/re-decode) rescues 21-50% of first-pass failures on level 2 (BPSK) under Watterson-Moderate/Poor (+6.0 dB at FER≤10% on Moderate), but only 1-2% on levels 5/6 (8PSK/16QAM) under Watterson-Poor. Plausibly connected to the still-open Task 1/7 estimation issue above (a first pass with overconfident wrong LLRs seeds virtual pilots with backwards-weighted confidence) but not confirmed — see `.superpowers/sdd/p2-task-5-report.md`
- **16-QAM's fast soft demapper (Task 6) is allocation-bound, not arithmetic-bound.** The closed-form per-axis min-reduction replacement for `Qam16Mapper`/`Qam64Mapper::demap_soft` is 19-162x faster in raw arithmetic (verified against a brute-force oracle), and 64-QAM's full production-API call clears a ≥8x speedup target (28-34x). 16-QAM's full-API call only reaches 4.2-4.3x: both the old and new code pay an identical, fixed `Vec<f32>` heap-allocation cost (via the shared `ConstellationMapper` trait) that dominates at 16-QAM's smaller workload (4 bits × 16 points) — not something the closed-form replacement itself can fix without a cross-cutting trait-signature change affecting every modulation in `coppa-codec`
- **Phase 2's cumulative full-ladder re-baseline (Task 8) does not cleanly clear the phase's own acceptance bar.** AWGN is met and exceeded (level 4 +3 dB, level 7 +3 dB, level 9 +6 dB at FER≤10%; level 10 fixed from non-convergent to clearing cleanly), and the soft header's failure share on Poor is met in aggregate (4.3%). But watterson-poor/level 2's "≥+3 dB" bar and watterson-poor/level 6's "≥+1.5 dB" bar are both **not measurable as literally specified** — Poor is an irreducible-outage-floor channel where neither the pre- nor post-Phase-2 codec ever crosses 10% FER at either level. The underlying (non-threshold) FER curves show level 2 substantially better at every SNR (roughly halved at 30 dB), a real win the literal metric can't express, but level 6 shows no such gain (flat to marginally worse). Watterson-Moderate is a genuinely mixed cumulative result: levels 1-2 (BPSK) improve at FER≤10% (turbo re-estimation's concentrated benefit outweighing the estimator regression), but levels 3-6 (QPSK, 8PSK, 16QAM 1/2) show a **real regression at matched SNR** (e.g. level 6 at 30 dB: 10.25%→40.0% FER), while levels 7/9 (16QAM 3/4, 64QAM 2/3) improve consistently. A previously-unexercised CFO×level-4 interaction also surfaced: level 4 (QPSK 3/4) under a 40 Hz carrier offset goes from clearing FER≤10%/≤1% to never clearing at all (peak goodput −46%) — not caught by any single dev task's own bench gate. **This regression has since been investigated and FIXED**, root-caused to CFO-induced sync-timing jitter (a few hundredths to ~0.36 grid units) desyncing Task 1's frame-global `calibrated_bias` reference just enough to leak real energy into a spurious second delay-domain tap and corrupt the LDPC input LLRs — level 4's tight rate-3/4 margin is the first to lose convergence entirely. Fixed by `CoppaModem::bounded_coarse_delay` (`COARSE_DELAY_JITTER_BOUND = 0.15`, commit `5e7ba93`): clamping the per-frame correction to a small window restores clean convergence across the whole CFO band (FER→0.00) while costing no measurable regression against the Watterson-fading case this same code area previously broke (98.3-98.5% vs. 98.8-99.0% baseline header pass rate, within ~1.2σ of trial noise) — independently reverified safe under combined CFO+Watterson-fading conditions (commit `daf1d45`). See `.superpowers/sdd/p2-cfo-level4-investigation-report.md` and `p2-cfo-level4-fix-report.md`. See `BENCHMARKS.md`'s "2026-07 — Phase 2 (parametric estimation + NR BG2): cumulative re-baseline" section for the complete per-level, per-channel data, and `docs/adr/006-phase2-parametric-estimation-nr-bg2.md` for the full decision record
- **A `CoppaCore` engine reliably fails to decode any real frame after its first successful decode — found as a side effect of the CP-switch peer-negotiation work (2026-07-29), not fixed, not root-caused.** After a `CoppaCore` instance's `push_samples` successfully decodes one real, `CoppaCore::encode_bytes`-built frame, it fails to decode any subsequent frame at all — even a bit-for-bit-identical copy of audio a *fresh* engine decodes fine on its first try. This surfaced only as a side effect of removing a test workaround for the handshake-deadlock bug documented above; it has nothing to do with CP negotiation itself. Ruled out during diagnosis: CP profile mismatch, the CpControl/ArqTx reply path (reproduces with `arq_enabled = false`), audio-ring corruption (samples proven bit-identical), speed level, and compression/frame-type/content specifics. Does **not** reproduce with raw-`CoppaTransceiver`-built ("warmup"-style) frames decoded repeatedly on the same instance — only frames built via `CoppaCore::encode_bytes` trigger it on a second decode. Root cause not isolated further; the leading (unconfirmed) hypothesis is `StreamingReceiver` internal state not resetting correctly after a decode, in some content/config-dependent way. Tony was asked whether to pause and investigate immediately or continue and document as a follow-up, and chose the latter. Full diagnostic detail (elimination method, exact repro, a candidate starting point) is in this feature's `.superpowers/sdd/task-5-report.md`. **UPDATE (2026-08-01): this turned out to be two separate, unrelated bugs, one now FIXED.** A full 9-level ladder sweep of the exact repro found level-dependent behavior: levels 2, 4, 5, 9 show a spurious sync artifact but still decode anyway; levels 1, 3, 6 are clean; levels 7 and 10 hard-fail, via two structurally different mechanisms. **Bug A (VHF levels 5-10, root-caused and fixed):** `select_ofdm_profile` routes every speed level >= 5 to `vhf_wide()` (`cp_samples = 60`, no TX bandpass filter), unlike `hf_standard()` (`cp_samples = 300`, has one). `SyncDetector`'s `local_peak_abs.saturating_sub(TIMING_BACKOFF)` timing-anchor computation silently saturates to 0 whenever a frame's preamble sits at buffer position 0 — true of every existing clean-loopback test AND of `CoppaModem::measure_bulk_bias`'s one-time calibration frame, but never true of a real received frame (which always has some nonzero leading margin except literally the first sample of a session). This desynced `calibrated_bias` from what real VHF decodes actually needed, breaking LDPC convergence — not really a "second decode" bug at all, just one that happened to only reproduce past a session's first frame. `TIMING_BACKOFF` itself was correct and unaffected; a CP-proportional-scaling attempt was tried first and disproven. Fixed by guaranteeing a fixed leading-silence margin before every `SyncDetector` call at all three real production consumers of `calibrated_bias` (`measure_bulk_bias`, `demodulate_frame_impl`, `StreamingReceiver::header_peek` — the third found only via full-suite testing). Two earlier fix attempts each looked fully green in isolation before `phase_c_loopback`'s Monte Carlo FER sweep and `coppa-cli`'s `rx_golden` suite caught a real regression — see the "Phase 2 Task 4 alpha-calibration process" bullet above for why this project treats that pattern as expected, not exceptional. Independently reviewed: the reviewer reproduced the exact `LdpcNotConverged` failure on the pre-fix commit and confirmed the VHF-only gating never touches HF's path. See PR #69 and `.superpowers/sdd/vhf-timing-backoff-fix-report.md` for the full investigation. **Bug B (HF level 7, narrowed, still open):** a spurious second sync candidate appears inside the inter-frame silence gap (garbage CFO, low cross-correlation), consuming buffer margin before the real candidate is reached. The leading hypothesis (scrambled zero-padding being low-diversity) was directly refuted — the scrambler's own existing test already confirms genuinely ~50/50 output even over padding. The real, evidence-backed trigger is the opposite: it's payload *size* that matters, not padding amount or cross-frame content similarity — larger payloads (near the level's LDPC capacity) fail, small ones don't. Next untested hypothesis: payload-dependent 16-QAM symbol amplitude interacting with per-level PAPR clipping, leaving a residual in `SyncDetector`'s persistent (never-reset-between-calls) AGC/correlation state. Not yet root-caused or fixed.

## Knowledge wiki

`wiki/INDEX.md` is the map of accumulated knowledge — read it before deep
exploration; open pages relevant to your task. After substantive work, run
/wiki-update: distill new gotchas/decisions/corrections into the wiki (or
into docs/ if normative — the wiki points, it never restates). The wiki is
descriptive and always loses conflicts with code and docs/.

## Multi-agent hygiene

You are never alone in this repo — other agents may be working concurrently
in other clones, branches, or worktrees.

- **Start fresh:** `git fetch` and rebase onto `origin/main` before reading
  code or making decisions; stale context produces wrong work.
- **Claim before work:** search open PRs/issues first; open a draft PR early —
  the draft PR *is* the claim. Don't duplicate in-flight work.
- **Isolate:** always a branch (worktree preferred), never a shared checkout's
  main. Use per-session scratch dirs; don't bind fixed ports.
- **Flush at the end:** push (`--force-with-lease` only) and open/update your
  PR before finishing. Unpushed work is invisible work.
- **Main moves only by PR merge.**
