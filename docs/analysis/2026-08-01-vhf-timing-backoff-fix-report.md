# VHF TIMING_BACKOFF fix report

## Step 1: what the mechanism actually is

`TIMING_BACKOFF = 30` (`crates/coppa-codec/src/ofdm/sync_detector.rs`) is a fixed
number of *samples* subtracted from the detected preamble position
(`local_peak_abs.saturating_sub(TIMING_BACKOFF)` in `SyncDetector::try_resolve_one`)
to give the downstream FFT window margin into the cyclic prefix.

**Confirmed mechanism (b) from the task's framing, not (a)**: this margin is an
absolute, sample-domain quantity whose resulting linear phase ramp across
subcarriers is independent of CP length (every profile shares `fft_size = 960`),
and is meant to be fully compensated downstream by `CoppaModem::calibrated_bias`
— measured ONCE per profile at construction (`measure_bulk_bias`) and applied to
every frame via `bounded_coarse_delay` (a tightly bounded, ±0.15 grid-unit
per-frame correction on top of the fixed `calibrated_bias`). A first attempt at
mechanism (a) — scaling `TIMING_BACKOFF` proportionally to `cp_samples` (30 for
HF's 300-sample CP, 6 for VHF's 60-sample CP) — was built and measured, and did
**not** fix the confirmed repro (still `LdpcNotConverged`), which is direct,
empirical evidence against (a) and for (b): the bound-of-6-samples ramp is not
what's failing; the calibration mismatch is.

**The actual bug**: `measure_bulk_bias`'s calibration frame is built with the
preamble starting at sample 0 of its buffer. For HF profiles, the 601-tap TX
bandpass filter's own ~300-sample group delay (`tx_bpf`) pushes the detected
timing anchor comfortably above 30 regardless, so `saturating_sub` never clamps
and `calibrated_bias` correctly captures the "backoff applied" state. **VHF
profiles have no TX bandpass filter** (`tx_bpf: None` for `phy_mode == 1`), so
the detected anchor for a position-0 buffer sits near 0, and
`local_peak_abs.saturating_sub(30)` silently clamps to 0 — the calibration frame
never actually exercises the deliberate backoff at all. `calibrated_bias` for
VHF ends up calibrated for the "no backoff" (saturated) case.

A REAL received frame handed to `demodulate_frame_impl`/`receive_with_metrics`
(explicitly documented, in both `streaming.rs` and `transceiver.rs`, to
"tolerate arbitrary leading margin/silence before the frame" via its own fresh
internal `SyncDetector::detect_all`) can have ANY amount of leading margin — 0
(as `StreamingReceiver`'s own slicing convention and most existing unit tests
use) or more. Whenever that margin is `>= TIMING_BACKOFF`, the real frame
engages the full, un-saturated backoff and its linear phase ramp — a ramp
`calibrated_bias` was never calibrated against for VHF.

## Step 2: failing test (RED)

Added `vhf_all_levels_survive_realistic_leading_offset` to
`crates/coppa-protocol/src/modem/transceiver.rs`: builds one `CoppaTransceiver`
per VHF-routed speed level (5, 6, 7, 9, 10), transmits, prepends 30 leading zero
samples (matching `TIMING_BACKOFF`), and asserts `receive()` succeeds.

Confirmed RED on unmodified `main` (verified via `git stash` of just the fix
files, test still present):

```
thread '...vhf_all_levels_survive_realistic_leading_offset' panicked:
VHF level 10 should decode a frame with a realistic non-zero leading offset,
got LdpcNotConverged { iterations: 60 }
```

## Fix attempts that did NOT work, in order (kept here because each one taught
## something the final fix depends on)

1. **Scale `TIMING_BACKOFF` proportionally to `cp_samples`.** Reverted —
   measured directly to not fix the repro even at the scaled-down value (6
   samples for VHF). `TIMING_BACKOFF` is unchanged at `30` in the final fix.
2. **Pad only `measure_bulk_bias`'s calibration frame** (unconditionally, all
   profiles) with `SYNC_LEAD_MARGIN` leading zero samples so its own detection
   no longer saturates. Fixed the original confirmed repro, but **regressed**
   the pre-existing `vhf_level5_transceiver_round_trips_with_bounded_peak` test
   (0/20, previously 20/20) — that test calls `receive()` on a position-0
   buffer, which still saturates on the real-decode side, now mismatched
   against a calibration that no longer saturates.
3. **Pad both `measure_bulk_bias` AND `demodulate_frame_impl`**, unconditionally
   for all profiles. Fixed both of the above (confirmed: full `coppa-codec` +
   `coppa-protocol` lib suites, `phase_c_loopback` incl. the Monte Carlo sweep,
   and `cargo test --workspace --lib` all green). **But this unconditional
   application to every profile — not just VHF — introduced two further,
   independently-confirmed regressions**, found only by running the true full
   suite (`cargo test --workspace`, not just `--lib`):
   - `crates/coppa-cli/tests/rx_golden.rs`'s `golden_vectors_rx_cli_decodes_and_prints_payloads`:
     all 6 VHF golden vectors (L5/L6/L9, clean+awgn12) started failing to decode
     via the CLI's `StreamingReceiver` path. Root-caused by direct debugging
     (temporary `eprintln!`, since removed) to a THIRD, previously-unaccounted-for
     consumer of `calibrated_bias`: `StreamingReceiver::header_peek`
     (`coppa-protocol/src/modem/streaming.rs`) calls
     `CoppaTransceiver::demodulate_header` with an explicit, externally-computed
     `data_start` — it does NOT call `SyncDetector::detect_all` itself (that's
     the whole point of a cheap "peek"), so it never got the same padding
     treatment and stayed in the old (unpadded) reference frame that
     `calibrated_bias` no longer matched, for VHF.
   - After padding `header_peek` too (`SYNC_LEAD_MARGIN` prepended to its own
     slice, `data_start` adjusted by the same amount) a NEW regression appeared
     on `L1_poor25` (an HF, Watterson-Poor golden vector) — confirmed this was a
     genuinely new, deterministic failure (re-verified: passes on unmodified
     `main`, fails with the unconditional-padding fix) most likely via
     `remove_cfo`'s per-sample phase reference (`-TAU*cfo_hz*i/sample_rate`, `i`
     relative to whatever buffer it's given) shifting by the pad amount for any
     frame with a nonzero (even noise-induced) sync CFO estimate — something HF
     profiles never needed protection against, since they already had ample
     margin.

## Step 3: the fix (final, landed)

`TIMING_BACKOFF` itself is **unchanged** (still `30`, still flat — confirmed
unnecessary and insufficient by attempt 1 above).

Two new `pub` constants in `crates/coppa-codec/src/ofdm/coppa_modem.rs`:
- `SYNC_LEAD_MARGIN = 4 * TIMING_BACKOFF` (120 samples): the minimum leading
  margin guaranteed before sync detection.
- `HEADER_PEEK_ANCHOR_MARGIN = SYNC_LEAD_MARGIN - TIMING_BACKOFF` (90 samples):
  the anchor position `header_peek` must use directly, since it cannot re-run
  detection to discover it itself.

Both are applied **only for profiles with no TX bandpass filter (VHF)** — every
padding site is gated on the existing HF/VHF discriminant already present in
each file (`self.tx_bpf.is_none()` in `coppa_modem.rs`,
`self.rx_group_delay == 0` in `streaming.rs`) — at all **three** call sites that
read `calibrated_bias`:

1. `measure_bulk_bias` (one-time per-profile calibration): for VHF only,
   prepend `SYNC_LEAD_MARGIN` zero samples to the calibration frame before
   detection, keeping every downstream index in that padded buffer's coordinate
   frame.
2. `demodulate_frame_impl` (real per-frame decode, underlying
   `CoppaTransceiver::receive`/`receive_with_metrics`): for VHF only, same
   treatment on whatever buffer the caller supplies, before running sync
   detection.
3. `StreamingReceiver::header_peek` (`coppa-protocol/src/modem/streaming.rs`):
   for VHF only, prepend `SYNC_LEAD_MARGIN` zero samples to its own slice
   (before CFO correction, matching `demodulate_frame_impl`'s order), and use
   `HEADER_PEEK_ANCHOR_MARGIN + rx_group_delay + 3*symbol_len` as `data_start`
   (rather than re-deriving the anchor via a fresh, costly `SyncDetector::detect_all`
   call, which would defeat the point of a cheap header-only peek).
   `HEADER_PEEK_ANCHOR_MARGIN`'s doc records an accepted, confirmed-safe
   approximation: `header_peek`'s own outer (session-wide) `SyncDetector` may or
   may not itself have saturated when it produced `start`, and there is no way
   to tell after the fact — but the header region is always plain BPSK
   regardless of speed level, and is confirmed (golden vectors re-tested with
   15/50/200/1000 extra leading silence samples, spanning both regimes) to
   decode correctly either way. The payload — which can be high-order QAM and
   is NOT tolerant of this — never goes through this path; it's demodulated by
   `demodulate_frame_impl`'s own fresh, fully self-correcting detection.

HF profiles are **completely untouched** by this fix (byte-identical code path
to before) — this is deliberate: they never had the underlying saturation
problem (their own TX/RX bandpass filter group delay already supplies ample
margin), and attempt 3 above proved that applying the padding unconditionally
to them anyway causes a real, separate regression.

### Files changed

- `crates/coppa-codec/src/ofdm/sync_detector.rs`: `TIMING_BACKOFF` made
  `pub(super)` (so `coppa_modem.rs` can reference its exact value) and its doc
  comment extended to record the confirmed VHF incident and point at where it
  was actually fixed. No behavioral change to the constant or its usage within
  this file.
- `crates/coppa-codec/src/ofdm/coppa_modem.rs`: added `SYNC_LEAD_MARGIN` and
  `HEADER_PEEK_ANCHOR_MARGIN` (both `pub`); VHF-gated padding in
  `measure_bulk_bias` and `demodulate_frame_impl`.
- `crates/coppa-protocol/src/modem/streaming.rs`: VHF-gated padding +
  `data_start` adjustment in `StreamingReceiver::header_peek`.
- `crates/coppa-protocol/src/modem/transceiver.rs`: added the regression test
  `vhf_all_levels_survive_realistic_leading_offset`.

## Step 4: broad validation (final state)

| Check | Result |
|---|---|
| New regression test (`vhf_all_levels_survive_realistic_leading_offset`, levels 5/6/7/9/10, leading offset = 30) | **PASS** |
| Pre-existing `vhf_level5_transceiver_round_trips_with_bounded_peak` (position-0) | **PASS** (20/20) |
| Pre-existing `test_transceiver_16qam_rate_1_2_loopback` (the `hf_standard` 16-QAM boundary test `TIMING_BACKOFF=30`'s doc comment cites) | **PASS**, unchanged |
| `test_transceiver_cfo_correction`, `hf_standard_header_survives_watterson_moderate_fading` | **PASS** (specifically re-checked given the CFO/padding-interaction risk identified in attempt 3) |
| `cargo test -p coppa-codec --lib` (incl. all 4 `sync_detector` tests) | **164 passed, 0 failed, 5 ignored** |
| `cargo test -p coppa-protocol --lib` | **336 passed, 0 failed, 4 ignored** |
| `cargo test --test phase_c_loopback --release` (full 1-10 speed ladder loopback + AWGN threshold tests) | **22 passed, 0 failed, 1 ignored** |
| `cargo test --test phase_c_loopback --release -- --ignored --nocapture test_snr_fer_monte_carlo` (~7200 frames, levels 1-10 across full swept SNR range) | **PASS** — FER=0.00 at every level except one 1/100 blip at level 1/2's lowest tested SNR, matching CLAUDE.md's documented baseline; no regression |
| `cargo test -p coppa-cli --test rx_golden` (the golden-vector regression suite via `StreamingReceiver`/CLI — the ONE test that caught both real regressions above) | **2 passed, 0 failed** (all 20 vectors, HF and VHF, clean/awgn12/poor25/ssbcfo) |
| `cargo test --workspace --lib` (all 13 workspace crates) | **All 0 failed** |
| `cargo test --workspace` (full suite incl. integration/proptest/CLI/golden-vector tests — the suite that caught both regressions) | **38/38 test binaries, 0 failed**, exit code 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | **Clean, no warnings** |
| `cargo fmt --all -- --check` | **Clean** |

## Concerns

- The fix is now scoped tightly to VHF (no TX bandpass filter), verified not to
  touch HF's code path at all. Confidence is high, but the process that got here
  is itself the cautionary tale: two rounds of "fixed by every test I ran"
  turned out to be false, each time only surfaced by finally running the true
  full suite (`cargo test --workspace`, not `--lib` or a hand-picked test list).
  Anyone extending this in the future should run the full suite, not a subset,
  before trusting a "looks fixed" result on this specific bug.
- `HEADER_PEEK_ANCHOR_MARGIN`'s approximation (documented in its own doc
  comment) is confirmed safe empirically (golden vectors with 15/50/200/1000
  extra leading samples), not derived from a formal bound — it relies on the
  header's own BPSK robustness to a small, constant reference-point error in
  the unsaturated-`start` case. This is a real, accepted approximation, not a
  latent bug, but it is worth knowing about if someone later changes the header
  FEC to something less robust than the current soft-ML + CRC-assisted list
  decode.
