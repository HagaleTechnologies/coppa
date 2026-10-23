# ADR-009: HF speed ceiling and the 0.2.0 ladder renumber

## Status

Accepted, COP-24, 2026-10-23.

## Context

64-QAM 5/6 fails through the 300-2700 Hz SSB filter on `hf_standard`
even at 60 dB SNR. COP-23 preserved the configured HF band during rate
adaptation, making this mode reachable on an HF engine.

The filter attenuates the 2650 Hz carrier by 2.54 dB and the 2700 Hz
carrier by 6.02 dB. Payload pilots stop at 2450 Hz. These measurements
support a band-edge explanation; they do not distinguish estimation
error from insufficient coding margin. The fresh FER sweep and the
engine-path acceptance test are recorded in BENCHMARKS.md under
"COP-24: 64-QAM 5/6 dropped from HF; ladder renumbered 1-9".

Tony Hagale's scope decision, updated 2026-10-15:

> drop level 10 on HF and renumber the ladder 1-9 in this ticket, once and
> cleanly: no aliases or reserved-number gaps kept for old numbers, since
> nothing is released. It's a wire-format break, so it takes a minor
> version bump (COP-93 #6).

## Decision

Keep nine modulation/rate pairs and close the unused wire-number gap.
Drop 64-QAM 5/6 from HF only. The numbering is:

| Modulation / rate | Before 0.2.0 | From 0.2.0 | HF allowed |
|---|---|---|---|
| BPSK 1/4 through 16-QAM 3/4 | 1-7 | 1-7 | yes |
| 64-QAM 2/3 | 9 | 8 | yes |
| 64-QAM 5/6 | 10 | 9 | no |

Values 0 and 10-15 are invalid. There are no old-number aliases or
reserved gaps. `VHF_FAST` retains 64-QAM 2/3, now level 8. VHF and
level-derived engines retain level 9; the latter select VHF at levels
5 and above. HF engines expose levels 1-8 through
`CoppaCore::available_speed_levels()`. Both `set_speed_level` and
`encode_bytes_at_level` reject level 9 on an HF-band engine. The daemon
constructs its rate controller from the engine's available levels.

The workspace and root package versions become 0.2.0. Header protocol
`version: 1` stays unchanged, following ADR-003 and ADR-005. The VARA TCP
greeting derives its version from the package. This is a deliberate wire
break above level 7; both peers must run 0.2.0 to use the full new ladder.
No dual decoding or compatibility negotiation is added.

## Rejected options and limits

Narrowing `hf_standard` to at most 2600 Hz and adding edge-carrier erasure
handling were alternatives in the ticket. The owner's decision chooses
the HF ceiling instead. Removing the mode globally would leave eight
modes and discard its VHF use, contrary to that decision.

The PAPR targets, LDPC parameters and correction-table values stay the
same. The stale `CodeRate::Rate7_8` metadata label for the 5/6 mode remains;
SPEC section 12 explains why it does not affect transmitted bits.
Constructor validation and RF-frequency gating remain outside this change.
An operator can still configure a VHF profile while using an HF radio.
The SSB evidence comes from deterministic in-process channel tests, not
a live two-radio field trial.

## Reading older results

BENCHMARKS.md's older sections, ADR-002 through ADR-008, and historical
review/result documents retain their original level numbers. Read their
level 9 as today's level 8, and their level 10 as today's level 9. The
new golden corpus replaces only the old L9 vectors with regenerated L8
vectors; L1, L2, L5 and L6 remain byte-identical.
