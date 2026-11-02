> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Radio-reality Phase 1: carrier offset + Newman preamble + TX conditioning

`feature/radio-reality` moves the waveform to occupy a realistic SSB passband instead of
the old full-Nyquist band, and along the way surfaced (and then repaired) a real
sensitivity regression. Documented here in full rather than only showing the final,
fixed state, because the regression's cause and the fix's limits are both worth knowing
for later dB-harvest work.

**What changed:**
1. **Carrier offset** (`carrier_offset=6`, `hf_wide` → 46 data carriers, was 50): active
   carriers now start at bin 6 (300 Hz) instead of bin 1 (50 Hz), keeping the whole
   occupied band inside a realistic ~300–2700 Hz SSB passband instead of starting at the
   very bottom of the audio band where a real radio's own highpass would destroy it.
2. **Newman-phase in-band preamble**: replaced the old full-Nyquist PN-BPSK comb with a
   Newman-phase comb confined to in-band even bins — real preamble energy now actually
   fits inside an SSB channel. The preamble is unit-RMS normalized by construction.
3. **TX conditioning** (per-section RMS leveling → RC-overlap → PAPR clip → HF-only
   601-tap bandpass → peak-normalize to 0.5 FS): added specifically to fix a regression
   the first two changes introduced (below).

**The regression (introduced by items 1–2, not caught by their own task reviews):**
those reviews checked spec compliance, symmetry, and independently re-derived the PAPR
math, but never ran a full AWGN acceptance sweep against baseline — so a real sensitivity
loss shipped undetected. Root cause: the bench's SNR convention (`awgn_ref_seeded`)
references injected noise to the *whole transmitted frame's* mean power. The Newman
preamble, being unit-RMS normalized while the header/payload body's sparse-bin IFFT
output is naturally much quieter, sits tens of dB hotter than the body with no leveling
in place — so the frame's mean power (and therefore the noise floor at any nominal SNR)
is dominated by the preamble, starving the payload of effective SNR. Measured on
`results/p1-post-task2/awgn.csv` (commit `eeea46d`, items 1–2 only, no TX conditioning
yet) against the pre-Phase-1 baseline (`results/hotfix-2026-07/awgn.csv`):

| Mode (level) | Baseline clears at | Post-Task-2 (no conditioning) clears at |
|---|---|---|
| BPSK 1/4 (1) | 12 dB | 24 dB (12 dB: 100% FER; 21 dB: 0.25%) |
| BPSK 1/2 (2) | 12 dB | 24 dB (12 dB: 100% FER; 21 dB: 0.25%) |
| QPSK 1/2 (3) | 12 dB | ~30 dB (12–21 dB: 100% FER; 24 dB: 82.25%) |
| QPSK 3/4 (4) | 12 dB | ~30 dB (9–24 dB: 100% FER; 27 dB: 52.75%) |
| 8PSK 2/3 (5) | 15 dB | **never clears in the −6…30 dB sweep** (100% FER throughout) |
| 16QAM 1/2 (6) | 15 dB | **never clears** |
| 16QAM 3/4 (7) | 18 dB | **never clears** |
| 64QAM 2/3 (9) | 27 dB | **never clears** |

A real, substantial regression — not the ~3 dB it first appeared to be from a partial
diff, but 9+ dB at the low end and complete decode failure at every tested SNR for every
mode above level 4.

**What the TX conditioning fix recovers** (`results/p1-task3-check/awgn.csv`, same seed
and trial count, all three fixes above applied):

| Mode (level) | Baseline clears at | Post-Task-2 clears at | Task-3-fixed clears at |
|---|---|---|---|
| BPSK 1/4 (1) | 12 dB | 24 dB | 15 dB |
| BPSK 1/2 (2) | 12 dB | 24 dB | 15 dB |
| QPSK 1/2 (3) | 12 dB | ~30 dB | 15 dB |
| QPSK 3/4 (4) | 12 dB | ~30 dB | 15 dB |
| 8PSK 2/3 (5) | 15 dB | never | **15 dB — matches baseline** |
| 16QAM 1/2 (6) | 15 dB | never | **15 dB — matches baseline** |
| 16QAM 3/4 (7) | 18 dB | never | **18 dB — matches baseline** (15 dB point actually *better* than baseline: 62.25% vs 94.75% FER) |
| 64QAM 2/3 (9) | 27 dB | never | **27 dB — matches baseline** |
| 64QAM 7/8 (10) | never really clears (96–99% FER at 27–30 dB even in baseline) | never | unchanged — this code rate is already at the edge of viability in every configuration |

Levels 5–10 (8PSK and above) are fully recovered to baseline parity or better. **Levels
1–4 (BPSK/QPSK, the most redundant, lowest-order modes) still clear 3 dB later than
baseline** (15 dB vs 12 dB) — a real, quantified, but much smaller residual gap than the
regression this fix repairs.

**Mechanism of the fix:** per-section RMS leveling (equalize preamble/probe/body to the
same RMS before the frame's mean power is computed) removes almost all of the
preamble-domination effect above; that leveling alone accounts for the entire recovery
shown here. The residual 3 dB at levels 1–4 is not yet root-caused further; it's left as
a known item for Phase 2's dB-harvest work (LLR calibration, real LDPC matrices) rather
than blocking Phase 1 on it, since Phase 1's purpose is radio-reality (a realistic
occupied band and preamble), not final dB optimization — and every mode still works at
every level, just at a slightly higher minimum SNR for the four most robust ones.

**A dead end worth recording so it isn't retried:** filtering a hard-clipped waveform
(the 601-tap linear-phase FIR applied after PAPR clipping) measurably regrows peaks via
ringing at the clip corners (~5 dB peak regrowth for a single clip→filter pass on
`hf_standard` level 2, confirmed by direct measurement). This looks like it should cost
average transmit power once the final stage normalizes to a fixed peak, and iterating
clip→filter a few times (the master roadmap's own anticipated "optional clip→filter
iteration") does measurably improve RMS-at-fixed-peak in isolation (~4.9 dB by 3
passes). It was implemented and benched, but produced **no net FER improvement** — small
mixed-sign differences at the noise floor of a 400-trial run (level 7 @ 15 dB: 249→245
errors, marginally better; level 9 @ 21 dB: 113→143 errors, worse; level 10: within a
few errors either way). This is expected, not a bug: `awgn_ref_seeded` references
injected noise to the actual transmitted signal's own mean power, so *any* pure
gain/peak-efficiency change is exactly self-cancelling under this bench's SNR
convention — whatever average power the conditioning chain produces, the noise floor
scales to match it at the same nominal dB. Iterative clip-filter would matter on real,
peak-limited hardware with a channel noise floor independent of transmit power; it is
invisible to (and reverted from) this codebase's own bench harness. Not implemented.

