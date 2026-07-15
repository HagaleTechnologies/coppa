---
id: adr-007-multi-codeword-frames
title: Why do multi-codeword frames retransmit whole, not per codeword?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/007-multi-codeword-frames.md
  - crates/coppa-protocol/src/**
verified:
  commit: 59b0b63
  date: 2026-07-14
links:
  - overview
  - coppa-protocol
  - adr-008-phase3-system-layer
---
Phase 3 added multi-codeword frames (up to `MAX_CODEWORDS=8` per frame, with
intra-frame cross-codeword interleaving) — a **wire-format break**. The
deliberate scope cuts are the thing to know: ACK addressing, turbo
re-estimation, and persistent IR-HARQ LLR combining were all NOT extended to
per-codeword granularity.

## Digest

A multi-codeword frame is retransmitted, if at all, as a whole (same `seq`,
cycling RV via the existing mechanism), not by `(seq, codeword-index)`. Turbo
re-estimation and IR-HARQ's persistent cross-retransmission LLR accumulator are
both scoped to `codewords <= 1` only, taking the exact pre-multi-codeword
decode path — and this boundary is enforced in code at the call sites, not just
asserted in the ADR. These are reasoned scope cuts (decisions 4–5 in the ADR),
not oversights: extending them per-codeword is future work with real wire and
state-machine implications.

If you see IR-HARQ combining or turbo rescue "mysteriously" not firing on a
large frame, check `codewords` first — that is the designed behavior.

Full reasoning: `docs/adr/007-multi-codeword-frames.md` (decisions 4–5).
