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

## Unreleased: 0.13.0

- **Crate version:** 0.13.0. Minor bump because `coppa_protocol::session::LinkCapabilities`
  gains a public field, `windowed_arq`, which breaks struct-literal callers that do not use
  `..LinkCapabilities::default()`.
- **Air-protocol version:** 1, unchanged. CONNECT capability flags bit 5 is now named
  `windowed_arq` (COP-168, SPEC §20.1). It is negotiated as an AND and CONNECT_CFM must
  carry exactly the AND (`ProtocolError::WindowedArqMismatch`). No release sets it yet,
  so every coppad still sends the 0.12.0 bytes.
- **Host-API version:** 1, unchanged.
- **C ABI version:** unchanged.
- **Rust API, additive:** `LinkCapabilities::windowed_window`, `arq::MAX_SESSION_WINDOW`,
  `arq::windowed_phy_seq`, `arq::windowed_harq_key`, `arq::phy_following`,
  `arq::RV_SCHEDULE`, `arq::rv_for_session_attempt` and `ArqTx::unacked`: the wire
  primitives for windowed session ARQ. The windowed sender and receiver are not built yet
  (`docs/LIMITATIONS.md`).

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
