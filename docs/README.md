# coppa documentation

Every document under `docs/`, grouped by who reads it. ADR files are listed in
[adr/README.md](adr/README.md), and benchmark records in [../BENCHMARKS.md](../BENCHMARKS.md).
When you add a file under `docs/`, add a line here.

## Start here

- [tutorials/getting-started.md](tutorials/getting-started.md): build coppa, run a loopback test, encode and decode a WAV file, and check your station.
- [tutorials/raspberry-pi-install.md](tutorials/raspberry-pi-install.md): run coppad as a service on a Raspberry Pi 4.
- [tutorials/embedding.md](tutorials/embedding.md): call the modem from your own Rust program.
- [OPERATING.md](OPERATING.md): field operation, including the startup report, PTT and TX level calibration.

## Connect a host

- [HOST-API.md](HOST-API.md): connect a host application to coppad over the VARA-style TCP ports, the WebSocket API or KISS.
- [PAT.md](PAT.md): run Winlink with Pat over coppa, and what that has and has not been tested against.
- [BPQ.md](BPQ.md): use LinBPQ as a coppa gateway.
- [ARDOP.md](ARDOP.md): the ARDOP host interface (COP-97).
- [RETICULUM.md](RETICULUM.md): carry Reticulum over the HF OFDM modem.
- [BROADCAST.md](BROADCAST.md): broadcast files over OFDM KISS.
- [TNC.md](TNC.md): the KISS TNC for AFSK 1200 and G3RUH 9600.
- [schema/websocket-protocol-v1.json](schema/websocket-protocol-v1.json): the JSON Schema for version 1 of the WebSocket protocol.

## Specification

- [SPEC.md](SPEC.md): the normative waveform, FEC and protocol conformance specification.
- [LIMITATIONS.md](LIMITATIONS.md): every measured gap, open bug and unmet acceptance bar.
- [../ARCHITECTURE.md](../ARCHITECTURE.md): the system design, the TX and RX pipelines, and what each layer does.
- [interop/mercury-air-protocol.md](interop/mercury-air-protocol.md): a clean-room description of the Mercury 1.9.23 air protocol and an interoperability assessment.

## Decisions

- [adr/README.md](adr/README.md): the index of design decisions (ADRs), with each one's status.
- [DECISIONS/2026-08-07-pr-review-convergence-policy.md](DECISIONS/2026-08-07-pr-review-convergence-policy.md): how review rounds converge instead of chasing finer findings.
- [DECISIONS/2026-09-15-native-merge-queue-cutover.md](DECISIONS/2026-09-15-native-merge-queue-cutover.md): the move from Mergify to GitHub's native merge queue.
- [DECISIONS/2026-10-14-docs-architecture.md](DECISIONS/2026-10-14-docs-architecture.md): where each kind of document lives, and why local working notes are never cited.
- [DECISIONS/2026-11-02-public-rust-api-semver-gate.md](DECISIONS/2026-11-02-public-rust-api-semver-gate.md): a public Rust API break bumps the minor version.
- [DECISIONS/2026-11-03-c-abi-versioning.md](DECISIONS/2026-11-03-c-abi-versioning.md): how the C ABI version is numbered, apart from the crate version.
- [DECISIONS/2026-11-03-codex-gate-auto-resolve-head-check.md](DECISIONS/2026-11-03-codex-gate-auto-resolve-head-check.md): the Codex review gate stops auto-resolving once the PR head moves.
- [DECISIONS/2026-11-09-clean-room-and-licensing-policy.md](DECISIONS/2026-11-09-clean-room-and-licensing-policy.md): work from public documentation only, and get the owner's approval before copying any code.
- [DECISIONS/2026-11-09-release-binaries-and-versioning.md](DECISIONS/2026-11-09-release-binaries-and-versioning.md): tagged pre-1.0 releases with prebuilt binaries.
- [DECISIONS/2026-11-14-github-repository-metadata.md](DECISIONS/2026-11-14-github-repository-metadata.md): the GitHub description, topics, homepage, Discussions and social preview, and the command an admin runs to apply them.

## Measurements

- [../BENCHMARKS.md](../BENCHMARKS.md): measured BER, FER and throughput for each speed level, with links to each record.
- [benchmarks/](benchmarks/): the dated benchmark records that BENCHMARKS.md summarises.
- [analysis/2026-07-03-world-class-gap-analysis.md](analysis/2026-07-03-world-class-gap-analysis.md): the July 2026 gap analysis against the state of the art in HF modems.
- [analysis/2026-08-01-vhf-timing-backoff-fix-report.md](analysis/2026-08-01-vhf-timing-backoff-fix-report.md): the VHF timing backoff fix report.
- [analysis/COP-118-fade-diversity.md](analysis/COP-118-fade-diversity.md): which fade-diversity mechanism keeps a coppa link up.
- [analysis/COP-134-fading-regressions.md](analysis/COP-134-fading-regressions.md): causes and a fix plan for the fading regressions.
- [analysis/COP-134-fading-evidence.csv](analysis/COP-134-fading-evidence.csv): the per-level FER measurements behind the COP-134 analysis.

## Verification records

- [testing/full-ci.md](testing/full-ci.md): the test groups the full CI run splits into.
- [testing/c-abi-check.md](testing/c-abi-check.md): the C ABI compatibility check (COP-178).
- [testing/cable-loop-turnaround.md](testing/cable-loop-turnaround.md): the cable-loop turnaround acceptance record (H-090 stage d).
- [testing/pat-session.md](testing/pat-session.md): a Pat Winlink session over coppa (COP-46).
- [testing/bpq-session.md](testing/bpq-session.md): a LinBPQ session over coppa (COP-172).
- [testing/tnc-direwolf-wa8lmf.md](testing/tnc-direwolf-wa8lmf.md): the AFSK 1200 TNC against Direwolf on the WA8LMF test CD (COP-112, COP-113).
- [testing/tnc-direwolf-g3ruh-9600.md](testing/tnc-direwolf-g3ruh-9600.md): the G3RUH 9600 comparison with Direwolf (COP-116).
- [testing/tnc-direwolf-il2p.md](testing/tnc-direwolf-il2p.md): IL2P interoperability with Direwolf 1.7.
- [testing/tnc-fx25-direwolf.md](testing/tnc-fx25-direwolf.md): FX.25 interoperability with Direwolf (COP-114).

## Reviews

- [reviews/2026-09-05-product-ux-market-review.md](reviews/2026-09-05-product-ux-market-review.md): the September 2026 product, UX and market review, which draws on the dimension reviews below.
- [reviews/2026-09-05-dim-A-cli-ux.md](reviews/2026-09-05-dim-A-cli-ux.md): dimension A, the `coppa` CLI.
- [reviews/2026-09-05-dim-B-daemon-ops.md](reviews/2026-09-05-dim-B-daemon-ops.md): dimension B, coppad configuration, deployment and operations.
- [reviews/2026-09-05-dim-C-host-apis.md](reviews/2026-09-05-dim-C-host-apis.md): dimension C, the host integration surfaces.
- [reviews/2026-09-05-dim-D-docs-positioning.md](reviews/2026-09-05-dim-D-docs-positioning.md): dimension D, documentation, onboarding and positioning.
- [reviews/2026-09-05-competitive-landscape.md](reviews/2026-09-05-competitive-landscape.md): dimension E, the competitive landscape and the jobs users hire a modem for.
- [reviews/2026-09-05-dim-F-modem-fitness.md](reviews/2026-09-05-dim-F-modem-fitness.md): dimension F, performance claims, robustness and on-air readiness.
- [reviews/2026-09-05-dim-G-rust-library.md](reviews/2026-09-05-dim-G-rust-library.md): dimension G, coppa as a Rust library.
- [reviews/2026-09-05-dim-H-missing-surfaces.md](reviews/2026-09-05-dim-H-missing-surfaces.md): dimension H, the product surfaces coppa does not have yet.

## Maintainers

- [RELEASING.md](RELEASING.md): how to cut a release.
- [../CONTRIBUTING.md](../CONTRIBUTING.md): how to contribute, and the rules for describing coppa.
- [../CHANGELOG.md](../CHANGELOG.md): what changed in each version.
- [../ROADMAP.md](../ROADMAP.md): open gaps, work in flight, and what is not started or on hold.
- [../SECURITY.md](../SECURITY.md): how to report a vulnerability privately.
- [../wiki/INDEX.md](../wiki/INDEX.md): short pages on gotchas and the reasons behind design decisions.
