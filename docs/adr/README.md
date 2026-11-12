# Architecture Decision Records

Design decisions for coppa, one file per decision. Process and owner calls live in
[`docs/DECISIONS/`](../DECISIONS/) instead.

| ADR | Title | Status | Decided | Relations |
|-----|-------|--------|---------|-----------|
| [ADR-001](001-workspace-layout.md) | Workspace Layout | Accepted | 2026-07 | Carries inline updates (COP-75) |
| [ADR-002](002-fec-strategy.md) | FEC Strategy | Superseded by ADR-005 | 2026-07 | ADR-005 replaced its LDPC family; its convolutional code is on no transmit or receive path |
| [ADR-003](003-phase1-waveform-break.md) | Phase 1 waveform break — carrier offset, Newman preamble, TX conditioning, streaming sync, two-stage CFO | Accepted, partly superseded | 2026-07 | ADR-004 corrects its root-cause speculation on the Watterson-fading regression; [ADR-012 (COP-128)](012-cop128-cfo-hypothesis-search.md) widens its ±50 Hz CFO acquisition range |
| [ADR-004](004-strongest-path-timing.md) | Strongest-path (not first-path) sync timing anchor for HF multipath | Accepted | 2026-07 | Corrects part of ADR-003 |
| [ADR-005](005-nr-bg2-ldpc.md) | NR BG2 mother code + circular-buffer rate matching + layered NMS decoder | Accepted | 2026-07 | Supersedes ADR-002; ADR-006 records it at phase level |
| [ADR-006](006-phase2-parametric-estimation-nr-bg2.md) | Phase 2 — parametric channel estimation + NR BG2 mother code | Accepted, partly superseded | 2026-07 | Its decode-CPU claim is superseded by ADR-005 and the [COP-6 log](../benchmarks/2026-08-03-cop-6-ldpc-decode-cpu.md) (ADR-006 line 146) |
| [ADR-007](007-multi-codeword-frames.md) | Multi-codeword frames + intra-frame cross-codeword interleaving | Accepted | 2026-07 | Cross-referenced by ADR-008 decision 6 |
| [ADR-008](008-phase3-system-layer.md) | Phase 3 — system layer (payload integrity, ARQ, IR-HARQ, rate loop, multi-codeword frames, SCO tracking, telemetry, benchmark program) | Accepted, partly superseded | 2026-07 | Its `BUFFER` 3,2,1,0 progression (COP-30 update, line 372) and the `session` zero-drop aspiration (COP-5 update, line 442) are superseded |
| [ADR-009](009-cop24-speed-ladder-renumber.md) | HF speed ceiling and the 0.2.0 ladder renumber | Accepted | 2026-10-23 | Renumbers the levels used in ADR-002 to ADR-008 |
| [ADR-010](010-cop38-session-arq.md) | Connected-session ARQ and the 0.3.0 session wire break | Accepted | 2026-10-27 | Follows ADR-009's level numbering and the ADR-003/005 no-negotiation precedent |
| [ADR-011 (COP-126)](011-cop126-decimated-sync-prefilter.md) | A decimated sync pre-filter on the streaming receive path | Accepted | 2026-11-03 |  |
| [ADR-011 (COP-139)](011-cop139-library-link.md) | A sans-IO library link in coppa-engine, exposed to C and Python | Accepted | 2026-11-03 |  |
| [ADR-012 (COP-119)](012-cop119-session-outer-code.md) | negotiated session erasure protection | Accepted | 2026-11-06 (merge date) |  |
| [ADR-012 (COP-128)](012-cop128-cfo-hypothesis-search.md) | Fine-stage CFO hypothesis search | Accepted | 2026-11-04 | Widens ADR-003's ±50 Hz acquisition range |
| [ADR-013 (COP-121)](013-cop121-session-app-messages.md) | chat as a negotiated second stream inside the session ARQ | Accepted | 2026-11-08 (merge date) |  |

The status column uses three values: `Accepted`, `Accepted, partly superseded` (a later ADR or
an inline update replaces one claim, and the rest stands) and `Superseded by ADR-N` (the
decision no longer describes the shipped system). Each ADR's own `## Status` section and its
dated inline updates are the record; this table summarises them.

"Decided" for ADR-001 to ADR-008 is month-level: their text carries no decision date, and the
shallow history cannot recover one. ADR-009 through ADR-012 (COP-128) give dates in their status lines where available; the two records without dates use labelled merge dates.

**Level numbers.** ADR-002 to ADR-008 use the speed-level numbers from before COP-24. Read
their level 9 as today's level 8 and their level 10 as today's level 9;
[ADR-009](009-cop24-speed-ladder-renumber.md) has the mapping.

## Number collisions

ADR-011 and ADR-012 each name two decisions. Cite them by filename.
ADR-011 means [COP-126](011-cop126-decimated-sync-prefilter.md) in
`crates/coppa-protocol/src/modem/streaming.rs` and ADR-003, but
[COP-139](011-cop139-library-link.md) in `ARCHITECTURE.md`, `docs/LIMITATIONS.md`
and the C FFI wiki page. ADR-012 means [COP-119](012-cop119-session-outer-code.md)
in `docs/OPERATING.md`, but [COP-128](012-cop128-cfo-hypothesis-search.md) in `docs/LIMITATIONS.md`.

ADR-007 and ADR-008 cite `crates/coppa-bench/examples/`, moved to
`diagnostics/` in COP-90. Those ADRs remain unedited as records.

## Adding an ADR

The next free number is 014. Claim it by adding your row to this table in your draft pull request. Start the file with `# ADR-NNN: <title>` and a
`## Status` section whose first line is the status, in the form ADR-009 and ADR-010 use:
`Accepted, <ticket>, <YYYY-MM-DD>.` Then add a row to the table above. To change an existing
decision, add a dated inline update (`**<ticket> update (<YYYY-MM-DD>):** ...`) to that ADR,
change its status line if the decision as a whole no longer holds, and update its row here.
Cite tracked files only: ADRs, `docs/`, `BENCHMARKS.md` and pull requests.
