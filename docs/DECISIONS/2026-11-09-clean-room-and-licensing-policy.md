# Clean-room and licensing policy: public documentation only, no copyleft or proprietary modem code, owner approval for any copied code

Recorded by COP-174 on 2026-11-09.

## Context

After a compatibility-focused open modem shut down, the first question hams ask about a new
modem is where it stands legally. coppa's answer was already sound. Its code is MIT OR
Apache-2.0, its waveform and protocol have a published specification, its host interfaces
are TCP command sets that other software already speaks, and it does not transmit any
proprietary modem's waveform. But none of this was written down as policy, and nothing told a
contributor, person or agent, which sources they may work from. This entry is the one place
that states both. `README.md`, `CONTRIBUTING.md`, `docs/HOST-API.md`, `CLAUDE.md` (and
`AGENTS.md`, a symlink to it) and `.github/pull_request_template.md` state it briefly and
link here.

## Decision

1. **What coppa is.** coppa is an open-source OFDM modem and Rust library for amateur radio
   on HF and VHF. Its code is licensed MIT OR Apache-2.0 (`Cargo.toml`, `LICENSE-MIT`,
   `LICENSE-APACHE`). Its waveform, FEC and protocol are published as a normative, versioned
   specification, [docs/SPEC.md](../SPEC.md), detailed enough to build an interoperable
   implementation without reading the Rust source. Host programs such as Pat drive `coppad`
   over the de-facto TCP TNC command sets they already speak: VARA-style, ARDOP, AGWPE and
   KISS ([docs/HOST-API.md](../HOST-API.md)).
2. **No RF compatibility with any proprietary modem, by policy.** coppa will not implement
   the waveform or on-air framing of VARA, PACTOR or any other proprietary modem.
   Contributors do not capture or analyse those modems' signals to reproduce them. Open,
   publicly specified on-air modes are allowed, and coppa already ships AFSK 1200 AX.25,
   G3RUH 9600, FX.25 and IL2P. The host TCP command sets are API shapes built from published
   documents. They imply no RF compatibility, and both ends of an OFDM link run coppa.
3. **Implement from public documentation only.** Public documentation means standards (for
   example 3GPP TS 38.212 and the AX.25 specification), published papers, published protocol
   and command documents (for example EA5HVK's "VARA Protocol Native TNC Commands"), and
   coppa's own [spec](../SPEC.md).
4. **Never read, port or copy copyleft or proprietary modem code.** This covers the source of
   any modem, TNC or radio-protocol implementation under a copyleft licence (GPL, AGPL, LGPL)
   or a proprietary one, such as Direwolf, srsRAN, VARA or PACTOR. You may run an unmodified
   program as a black-box test oracle outside this repository and commit its outputs with a
   note of where they came from: the program, its version and its licence. From this date
   you may not build a patched copyleft checkout, because patching it means reading its
   source.
5. **Copied code from any source needs the owner's explicit approval first.** This includes
   permissively licensed projects and other Hagale Technologies repositories. Ask in a
   GitHub issue before you copy. The PR links the approval, names the source and its
   licence, and keeps the source's copyright notice.
6. **cargo-deny's licence allowlist is the enforcement point.** The `[licenses]` section of
   `deny.toml` (lines 45-68) lists the licences a dependency may carry. The "Supply-chain
   check (cargo-deny)" job (`.github/workflows/ci-full.yml:199-206`) fails a PR that adds a
   dependency outside the list. MPL-2.0 is on the list (`deny.toml:59`). It is file-level
   copyleft that applies to the dependency's own files. Adding a licence to the list is an
   owner decision. The check reads dependency metadata, not source files, so it does not see
   pasted source. COP-174 tested both cases on 2026-11-09 with cargo-deny 0.20.2. A source
   file with a `GPL-2.0-or-later` header pasted into `crates/coppa-dsp/src/` passed
   `cargo deny check licenses`. A path dependency declared `license = "GPL-3.0-only"` failed
   it with `rejected: license is not explicitly allowed`. The PR template's clean-room
   confirmation and review cover source provenance.
7. **Agents follow the same rules.** `CLAUDE.md`, which `AGENTS.md` links to, carries the
   short form. `gh pr create --body` skips the PR template, so an agent pastes the
   template's checklist into the body it writes.

## Prior practice

These items predate this policy. They are listed so the rules do not read as a claim that
nothing of the kind happened. Each item says what its record states. The rules apply from
2026-11-09 and do not reopen this work.

- `tools/gen_nr_bg2/src/main.rs:10-33`: the BG2 LDPC base-graph table was cross-checked
  against srsRAN Project (AGPL-3.0) and NVIDIA Sionna (Apache-2.0). The comment says the
  197 shift values are "copied verbatim from that cross-checked intersection". They are the
  values of 3GPP TS 38.212 Table 5.3.2-3, a standard's data table, not implementation code.
- `crates/coppa-codec/src/afsk.rs:177-179` (COP-113): the comment cites the receiver
  constants to WB2OSZ's paper as Direwolf's profile A values, "used as numbers only".
  COP-113's research records that the implementer read Direwolf's source to learn the
  stages and constants, and that no Direwolf code was copied.
- `crates/coppa-host/src/agw.rs` (COP-137, COP-164): COP-137's research read Direwolf's
  `server.c` for protocol behaviour before the AGWPE server was written, and told the
  implementer not to copy its code. `docs/LIMITATIONS.md:314-315` records one value, the
  9600 baud code in the `g` reply, taken from a `server.c` comment (COP-164).
- `tools/fx25-interop/README.md` and `testdata/il2p/README.md`: the fixture generators
  patch a scratch Direwolf checkout. Both READMEs say no Direwolf implementation code is
  copied into this repository. Decision 4 now rules that method out, so regenerating
  fixtures that way needs the owner's explicit approval as an exception.
- `tests/codex_gate_auto_resolve_head.rs:9`: the comment says the workflow's head re-check
  was "ported from propagation f8e1243", a Hagale Technologies repository. Decision 5 now
  puts porting code from another repository behind the owner's approval.

## Not done yet

- Nobody has audited the prior-practice items for copied expression.
- GitHub's licence sidebar shows only Apache-2.0 (`gh repo view` reports `apache-2.0`). The
  README and `Cargo.toml` are where MIT OR Apache-2.0 is stated.
- `ARCHITECTURE.md`, `docs/PAT.md`, `docs/ARDOP.md` and `crates/coppa-host/src/vara/mod.rs`
  still describe the VARA relationship in their own words and do not link here.
- `tools/fx25-interop/README.md` and `testdata/il2p/README.md` still describe regenerating
  fixtures from a patched Direwolf checkout and do not point at decision 4.

## Rejected alternatives

- **A rule that names only proprietary modems.** It misses the GPL and AGPL stacks that the
  repository has actually consulted (see Prior practice).
- **Relying on cargo-deny alone.** It cannot see pasted source (decision 6).
- **A CI job that checks the PR body for the clean-room box.** Agents write their own PR
  bodies, so the job would check text the author controls, and it would be new gating
  plumbing for a rule that review already covers.
