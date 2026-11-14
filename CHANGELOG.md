# Changelog

Release notes for the coppa workspace. Each release states four version
numbers separately, because they move independently:

- **Crate version**: the semver of every workspace crate and of `coppa --version`.
- **Air-protocol version**: the on-air format, carried in the PHY and MAC
  headers. `docs/SPEC.md` §0.3 states what version 1 freezes.
- **Host-API version**: the TCP command port and WebSocket API
  (`docs/HOST-API.md`).
- **C ABI version**: `coppa.h` and the `coppa-ffi` library
  (`docs/DECISIONS/2026-11-03-c-abi-versioning.md`).

Older releases have no entry here. Their on-air breaks are listed in
`docs/SPEC.md` §0.1.

## Unreleased: 0.16.0

- Add negotiated low-rate floor sessions to the Rust library Link. Discovery
  starts on OFDM, falls back after two unanswered windows, and probes OFDM
  after five distinct acknowledged floor exchanges. Set
  `LinkConfig.robust_floor = false` for the previous OFDM-only behavior.
- Expose the last transmitted and validated received modes through
  `Link::phy_mode()`. Host mode reporting remains open.
- coppad negotiates the low-rate floor for its handshake (COP-202). In Auto it
  advertises bit 6, sends a floor CONNECT_REQ after two unanswered OFDM
  windows and answers in the received waveform, so two daemons connect at
  −6 dB AWGN (10/10 recorded). It reuses the library's `FloorController`.
  Session data after a floor handshake still keys on OFDM.
- An unset VARA `RETRIES` now means 3 additional connect requests while the
  speed level is automatic (none with a pinned level). An explicit
  `RETRIES n` is honoured exactly.
- **Rust API, additive:** `FloorController::floor_active` and the shared
  floor policy functions `admits_floor_pdu`, `floor_caps_unusable`,
  `floor_segment_limit`, `floor_rto_floor`, `floor_keepalive_interval` and
  `calling_handshake_timeout` in `coppa_protocol::floor_controller`.
- Allocate capability flags byte bit 6 without changing legacy bytes or
  capability length.
- **Crate version:** 0.16.0. Minor bump over 0.15.0 because
  `coppa_engine::LinkConfig` and `coppa_protocol::session::LinkCapabilities` each gain a
  public field, `robust_floor`, which breaks struct-literal callers that do not end with
  `..LinkConfig::new(callsign)` or `..LinkCapabilities::default()`.
- **Air-protocol version:** unchanged (1, plus 2 for the floor from 0.14.0). In version 1,
  CONNECT capability flags bit 6 is now named `robust_floor` (SPEC §20.1). It is negotiated
  as an AND and CONNECT_CFM must carry exactly the AND
  (`ProtocolError::RobustFloorMismatch`). The library Link sets it by default; coppad sets
  it while its speed level is automatic (flags `0x46`, `0x56` with the WebSocket API on,
  `0x42` with `fade_protection = "off"`).
- **Host-API version:** 1, unchanged.
- **C ABI version:** unchanged.

## Unreleased: 0.15.0

- **Crate version:** 0.15.0. Minor bump over 0.14.0 (COP-190) because
  `coppa_protocol::session::LinkCapabilities` gains a public field, `windowed_arq`, which
  breaks struct-literal callers that do not use `..LinkCapabilities::default()`.
- **Air-protocol version:** unchanged (1, plus 2 for the floor from 0.14.0). In version 1,
  CONNECT capability flags bit 5 is now named
  `windowed_arq` (COP-168, SPEC §20.1). It is negotiated as an AND and CONNECT_CFM must
  carry exactly the AND (`ProtocolError::WindowedArqMismatch`). No release sets it yet,
  so every coppad still sends the 0.12.0 bytes.
- **Host-API version:** 1, unchanged.
- **C ABI version:** unchanged.
- **Rust API, additive:** `LinkCapabilities::windowed_window`, `arq::MAX_SESSION_WINDOW`,
  `arq::windowed_phy_seq`, `arq::windowed_harq_key`, `arq::phy_following`,
  `arq::RV_SCHEDULE`, `arq::rv_for_session_attempt`, `ArqTx::unacked` and the
  `ProtocolError::WindowedArqMismatch` variant: the wire
  primitives for windowed session ARQ. The windowed sender and receiver are not built yet
  (`docs/LIMITATIONS.md`).
- **Rust API, additive (receive-side HARQ):** `modem::HarqPolicy`,
  `CoppaTransceiver::{set_harq_policy, harq_policy, harq_combined_successes}`,
  `StreamingReceiver::{set_harq_policy, harq_policy, harq_combined_successes,
  take_failed_headers}`, `streaming::MAX_FAILED_HEADERS` and
  `CoppaCore::{encode_bytes_tagged, set_harq_policy, take_failed_headers,
  harq_combined_successes}`. The default policy keeps the old combining.
- **Behaviour change:** a soft-combining buffer now restarts when a frame under the same
  key has a different payload length, not only a different level, so two frames of
  different lengths that share `seq_num` 0 no longer add their soft bits together.

## Unreleased: 0.14.0

COP-190 adds the low-rate floor as a production air format, framed at
air-protocol version 2. It is a separate format from the OFDM speed
ladder: nothing a version-1 station sends or parses changes.

- **Crate version:** 0.14.0 (minor: new public modules).
- **Air-protocol version:** 2 allocated for the floor, in addition to the
  unchanged version 1. SPEC §0.3.3 registers the allocation and
  `docs/adr/014-cop190-low-rate-floor.md` freezes the framing: 16-FSK, 50
  baud, 600–1350 Hz, 4-byte header (version, length, CRC16) repeated four
  times and decoded jointly, NR BG2 LDPC payload with CRC32, 40-byte PDU
  in a fixed 8.0 s airtime. New `coppa_codec::floor` (modulation) and
  `coppa_protocol::modem::floor` (`FloorFrame`, `FloorStream`,
  `FloorError`, `FLOOR_AIR_PROTOCOL_VERSION`), guarded by
  `air_v2_conformance.rs` and `floor_receive.rs`.
- **Host-API version:** 1, unchanged. The `phy_mode` status field and the
  floor-aware session controller are not built yet; see
  `docs/LIMITATIONS.md`, COP-169 section.
- **C ABI version:** unchanged; no exported symbol or layout change.

## Unreleased: 0.12.0

The first release covered by the air-protocol version 1 compatibility
contract (COP-178). A 0.12.0 station and any later release that still sends
air-protocol version 1 interoperate on every item in SPEC §0.3's freeze table.
Releases before 0.12.0 also sent version 1 but are not covered.

- **Crate version:** 0.12.0.
- **Air-protocol version:** 1, unchanged. No on-air field changed. SPEC §0.3
  now freezes the frame fields, FEC parameters and session messages of
  version 1 and names the test that guards each one.
- **Host-API version:** 1, unchanged. Additive only:
  - TCP: the new `VERSIONINFO` command answers
    `VERSIONINFO crate_version=0.12.0 air_protocol_version=1 host_api_version=1`.
    The greeting and the `VERSION` reply are unchanged. An older daemon answers
    `WRONG`.
  - WebSocket: the authenticated `hello` gains `crate_version`,
    `air_protocol_version` and `host_api_version`. The existing `version` and
    `server` fields are unchanged.
- **C ABI version:** 1.0 to 1.1, compatible additions only:
  `coppa_air_protocol_version()` and `coppa_host_api_version()`. No existing
  function, struct layout or constant changed, so binaries built against 1.0
  keep working. The Python binding exposes both as
  `coppa.air_protocol_version()` and `coppa.host_api_version()` and requires
  ABI 1.1.

## Template for an incompatible on-air change

SPEC §0.3 requires every release that changes a frozen on-air item to carry
this block. Never reuse an air-protocol version number.

```markdown
### Air-protocol break: version N to version N+1

- Old version: N. New version: N+1 (PHY header and MAC header).
- Incompatible fields: <SPEC §-numbers and field names>.
- First release that transmits version N+1: <crate version>.
- Peers: <must upgrade | this release keeps a version-N decoder until <release>>.
- Detection: a version-N receiver fails preamble acquisition on version N+1
  frames; it does not report the mismatch to its host.
```
