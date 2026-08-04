---
id: adr-005-nr-bg2-ldpc
title: Why did the LDPC layer move to a single NR BG2 mother code?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/005-nr-bg2-ldpc.md
  - crates/coppa-protocol/src/fec/ldpc/**
verified:
  commit: 59b0b63
  date: 2026-07-14
links:
  - overview
  - coppa-protocol
  - adr-002-fec-strategy
  - ldpc-non-convergence
---
Phase 2 replaced nine separate per-rate 802.11 QC-LDPC codes with one
5G-NR-style BG2 mother code (Zc=176 lifting, `NrLdpc`), rate-matched down per
speed level via a circular buffer (`rate_match.rs`) with a layered
normalized-min-sum decoder. Level 10's nominal rate moved 7/8 → **5/6**. This
is a **wire-format break**: pre-change frames are not decodable by the new
codec and vice versa.

## Digest

The NR codes are the product of a published density-evolution optimization and
outperform the 802.11 codes at comparable block lengths; a single mother code
also means one graph, one decoder, one rate-matching path instead of nine
parallel implementations. The change fixed the old levels-9/10
non-convergence-at-high-SNR bug outright (see [[ldpc-non-convergence]]).

Two measured gaps against the change's own acceptance targets were kept open
rather than hidden: (1) the coding gain at matched rate/block-length is real
but smaller than predicted (+0.5 dB measured vs ~1.8 dB predicted at level 2;
a layered-vs-flooding A/B ruled out a decoder-schedule bug — believed to be a
finite-length effect); (2) after COP-6's safe contiguous sub-row rewrite and
271 KiB index-table deletion, decode CPU/frame is 1.48–4.33x the old codec
(levels 1/2/3/4/5/6/7/9/10: 1.48/3.45/1.69/2.29/4.33/3.34/2.54/3.66/2.80x;
budget ≤3x; levels 1/3/4/7/10 met, 2/5/6/9 did not). The worst is level 5,
whose old codec used rate 2/3.
The shared graph no longer shrinks at high rates; a measured scalar follow-up
regressed and was reverted, leaving a structural safe-portable ceiling.

## The alpha-calibration trap

The normalized-min-sum scale (alpha) was once picked from a sweep at **one**
speed level (2); it improved that level and broke level 10 to 100% frame loss
on a clean channel (highest rate, least redundancy, heaviest known-pad pinning).
Any future alpha/decoder-parameter change must be validated across the whole
speed ladder and payload-size extremes before shipping as the default.

Full rationale, the audited `k_used` table, and the i_LS cross-validation:
`docs/adr/005-nr-bg2-ldpc.md` and `.superpowers/sdd/p2-task-4-report.md`.
