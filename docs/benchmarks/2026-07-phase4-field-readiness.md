> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Phase 4 (field readiness): what changed vs. the 2026-07-03 world-class analysis

`docs/analysis/2026-07-03-world-class-gap-analysis.md`'s §4 ("real-world utility gaps —
what actually blocks field adoption") ranked six operator-facing gaps by impact, ahead of
any dB/FER item, on the reasoning that these — not modem sensitivity — were what separated
ARDOP-class from VARA-class deployments in the only public head-to-head comparison it found.
§7 explicitly proposed these become "Phase 4 (field readiness)". Phase 4 is now complete;
here is each of that list's six items against what actually shipped, plus the two §6
benchmark-program items it also named.

1. **"No telemetry ever reaches a client"** — already fixed before Phase 4 started (Phase 3
   Task 7 wired real VARA `SNR`/`PTT`/`BUFFER`/`BUSY` responses and a live WebSocket
   `status` reply). Not a Phase 4 item; listed here only to close the loop on the analysis's
   own numbering.
2. **"No TX level calibration"** → **Task 1 (TUNE)**: shipped. Two-tone (700+1900 Hz) and
   single-tone TX test signal, peak-normalized to the shared `TX_PEAK` constant, keyed via
   PTT through both `coppa tune` (CLI) and a daemon VARA `TUNE`/`TUNE <seconds>` command;
   `docs/OPERATING.md` documents the ALC calibration procedure. One real review-caught
   defect fixed before merge: the CLI implementation initially duplicated ~85 lines of PTT/
   device-sequencing logic between `cmd_tx` and `cmd_tune` — extracted into shared
   `write_wav_output`/`transmit_live` helpers, verified line-for-line to introduce no
   behavioral drift.
3. **"PTT stubs ... fall back to `NullPtt` silently; CLI hardcodes rigctld address and
   lead-in times"** → **Task 2**: shipped. Real DTR/RTS serial PTT and Linux sysfs GPIO PTT,
   both genuinely loopback-mock-tested against a real hardware-write trait (not
   "doesn't-panic" stubs); `serialport` added as the phase's one new dependency, gated
   behind `serial-ptt` (GPIO needed no new dependency, plain `std::fs`). The daemon's PTT
   construction is now `Result`-returning and **hard-errors at startup** on any
   unrecognized/unimplemented PTT config value — `NullPtt` is reachable only via an
   explicit `ptt = "none"`, closing exactly the "falls back silently" gap the analysis
   named. CLI gained `--rigctld host:port --ptt-lead-ms --ptt-tail-ms`, proven (via a real
   `TcpListener` test, not just parsed-and-dropped) to actually reach construction.
4. **"No busy-channel detection ... no station ID timer ... no beacon/CQ mode ... no
   waterfall data ... `coppa rx` prints 'not yet implemented'"** — the analysis's single
   largest bundled item, split across **Tasks 3 and 4**: all five sub-gaps shipped.
   Busy-channel courtesy (defer-and-random-0.5–2s-holdoff, re-checked after the hold, built
   on the spectral-occupancy `BusyGate` the analysis noted already existed unused);
   9-minute station-ID timer and beacon mode (both gated on a configured `callsign`,
   FCC §97.119-informed default); a 128-bin/300–2800 Hz/4 Hz WebSocket waterfall stream,
   opt-in per client (the first per-client-filtered content on a broadcast channel that
   previously had none); and `coppa rx` rewired onto the raw-bytes `StreamingReceiver`
   path for both live-device and file input, replacing the one-shot, UTF-8-forcing batch
   decode the analysis's "not yet implemented" line was describing. Two real, review-caught
   defects along the way, both fixed before merge: (a) the busy-wait's audio-observation
   loop initially routed through full protocol dispatch, letting a decoded CONNECT_REQ
   mid-wait trigger a nested, ungated `transmit_samples` call — fixed by narrowing the
   busy-wait's audio pump to feed only the occupancy gate, structurally unable to reach
   dispatch; (b) `header_peek` (the streaming receiver's fast pre-check) was missing CFO
   correction before header demod, found while proving `coppa rx` against CFO golden
   vectors — fixed to mirror the batch path's existing pre-header CFO-removal gate exactly,
   with both a targeted regression test and end-to-end golden-vector proof. Also
   discovered and fixed mid-phase, not part of the analysis's own list but squarely a
   field-readiness defect: the daemon's real streaming-decode path forced UTF-8 on every
   decoded payload before attempting MAC-PDU parsing, so real binary/compressed session
   traffic (the daemon's actual ARQ/CONNECT_REQ dispatch) almost always failed to
   validate as UTF-8 and was silently dropped — meaning real over-the-air session
   establishment through the daemon's true audio pipeline had likely never worked, missed
   by Phase 3's own `session`/`milstd` benches because they drive `ArqTx`/`ArqRx` directly
   and never exercise the real decode path. Fixed (`StreamFrame::message: Result<String>`
   → `payload: Result<Vec<u8>>`, no UTF-8 forcing) with regression tests proving a real
   binary `MacPdu` now round-trips intact.
5. **"Zero golden test vectors ... no waveform conformance spec ... no OTA methodology"** —
   the golden-vector corpus (20 WAVs + manifest) shipped in Phase 3 Task 8, ahead of Phase 4.
   The conformance spec is Phase 4's **Task 5**: `docs/SPEC.md`, ~1024 lines covering every
   section the analysis implied a second implementer would need (sample rate/FFT/CP/symbol
   timing, carrier map, preamble, probe PN, pilots, header + FEC layout, NR BG2 constants +
   rate matching, interleavers, multi-codeword framing, TX conditioning, byte/bit orders),
   each normative claim cited to its defining code symbol. This task's own acceptance
   criterion was an independent reviewer cross-checking every citation against source — an
   unusually thorough pass (the document's own authorship was atypical: one of the
   implementer's dispatched research sub-agents exceeded its scope and wrote the whole
   document itself, which is exactly the kind of path most likely to let a wrong-but-
   plausible number slip through) found and the coordinator fixed **one real, wire-breaking
   defect**: the header's stride-5 interleave formula had the permutation direction
   inverted (it stated `interleave`'s mapping using what was actually `deinterleave`'s
   formula) — an implementer building a from-scratch transmitter against the original text
   would have produced a header the reference receiver could not decode. Every other
   checked claim, across a large representative sample spanning all required sections,
   matched source exactly.
6. **"FFI is not integrable: text-only API, no binary payloads, no config surface, no
   metadata"** → **Task 6**: shipped. New `coppa_engine_new_with`/`coppa_engine_set_speed_level`/
   `coppa_encode_bytes`/`coppa_engine_feed_samples`/`coppa_next_frame` surface (config,
   binary TX, and event-style binary streaming RX with SNR/CFO/speed_level/a synthetic
   per-handle `seq`, all unified on one handle), additive — the old text-only quartet
   (`coppa_start_stream`/`coppa_feed_samples`/`coppa_get_decoded`/`coppa_stop_stream`) is
   unchanged and kept as a deprecated wrapper, byte-for-byte diffed identical to confirm no
   behavior drift. One real naming collision the brief's own pseudocode didn't anticipate —
   its proposed new streaming-feed name, `coppa_feed_samples`, was already taken by the old
   quartet — resolved by naming the new one `coppa_engine_feed_samples`, matching the
   convention its own other new functions already used. The unsafe FFI ownership/free paths
   (frame-queue pop, binary-encode allocation) were traced for use-after-free/double-free
   risk specifically because this is unsafe C-ABI code; none found, both mirror the
   pre-existing `coppa_encode`/`coppa_free_samples` allocation convention exactly.

§6's benchmark-program items are also now both complete: item 3 (golden vectors) shipped in
Phase 3 Task 8; item 4 (conformance spec) is this phase's Task 5, above.

**What Phase 4 is not**: none of it moves a dB/FER number — it is the operational layer the
analysis argued should be built *without* being gated on further PHY work, and the full
release-mode workspace test suite (all crates, all features) was re-verified green at the
phase gate with no regressions to any of the existing FER/goodput tables below. One CI-facing
regression was caught and fixed at the phase gate itself, not by any individual task's own
review: `cargo clippy --workspace --all-targets -- -D warnings` (CI's exact bare invocation,
no feature flags) failed to compile — Task 4's new `coppa-daemon::spectrum` module was
referenced only from `websocket`-feature-gated code but its own module declaration wasn't
gated, so a default-feature build saw it as 7 dead-code errors. A one-line `#[cfg(feature =
"websocket")]` fix on both the `lib.rs` and `main.rs` module declarations resolved it,
re-verified clean under both the bare and features-enabled clippy invocations. Every prior
phase's task-level reviews had checked clippy only with the phase's relevant features
enabled, never the literal bare CI command — worth carrying into any future phase's task
dispatch instructions as an explicit, named check, not just a features-enabled one.

