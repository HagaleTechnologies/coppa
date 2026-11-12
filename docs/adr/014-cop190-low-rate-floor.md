# ADR-014: COP-190 low-rate floor — air-protocol version 2 framing

## Status

Accepted, COP-190, 2026-11-12.

## Context

COP-169 specified a robust low-rate floor below the OFDM ladder's lowest
level. COP-187/COP-189 built and measured a 16-FSK candidate inside
`coppa-bench` (`floor_candidate.rs`) but left it measurement-only: no
production crate could send or detect it, there was no allocated air
version, and COP-189's recheck mirrored that residual as COP-190
(`docs/LIMITATIONS.md`, COP-169 section). The measured facts carried over:
0/10 frame errors at −6 dB AWGN and headroom to −10 dB on the dev seeds;
header sync dominating near the Poor crossing, which stays an open RF
question recorded in BENCHMARKS.md's COP-169 sections.

## Decision

The floor becomes a production format at **air-protocol version 2**, the
second allocation under SPEC §0.3.3 (value 0 reserved, value 1 the OFDM
contract). It is a separate air format: no version-1 constant, vector,
serializer or C-ABI field changes.

The modulation lives in `coppa-codec` (`floor.rs`), the framing and
receiver in `coppa-protocol` (`modem/floor.rs`). The frozen parameters:

- 48 kHz sample rate; 50 baud (960-sample symbols); 16 orthogonal tones
  spaced 50 Hz from 600 Hz to 1350 Hz, inside the 300–2700 Hz SSB passband.
- 16-symbol sync alphabet `[0, 9, 4, 15, 2, 13, 6, 11, 1, 14, 7, 10, 3,
  12, 5, 8]`.
- Header: 4 bytes — version nibble pair `2`, length, CRC-16/IBM-SDLC over
  the first two bytes — repeated 4 times independently; decoded jointly
  over the legal length/CRC combinations with positive-evidence and
  runner-up requirements (COP-189's repair).
- Payload: NR BG2 LDPC (ADR-005), information length K = 352 bits,
  rate-matched to E = 1408 bits, one 4-bit tone per symbol with a fixed
  `(symbol*4+bit)*31 mod E` bijection; the information word carries the
  payload, zero padding to 40 bytes and a CRC-32/ISO-HDLC check. The
  non-punctured information tail is known zero (no scrambler).
- Capacity: `MAX_PAYLOAD = 40` bytes — the CONNECT Req/Ack/Cfm MAC PDUs
  fit one frame. Airtime is exactly 8.0 s (384 000 samples): sync + 32
  header symbols + 352 payload symbols.
- Retry budget: at most 7 floor frames inside a 60 s connect window.
- Receiver: sliding matched-filter bank over 43 bins (450–1500 Hz, 25 Hz
  steps) at all eight symbol phases; sync search over 13 CFO hypotheses
  (±150 Hz in 25 Hz steps, zero first), at most 16 separated sync
  candidates per hypothesis; delivery only after LDPC convergence, CRC-32
  match and zero-padding check. `FloorStream` decodes one window per
  second and retains at most `CAPTURE_LIMIT` = frame + 2 s of audio.
- CPU/memory bounds: the oscillator bank is ~43 × 1920 complex templates
  plus 16 real render waveforms (~0.72 MB), built once; a decode attempt
  is one matched-filter pass over the retained window plus 13 hypothesis
  views of it.

## Consequences

- `coppa-codec::floor` and `coppa_protocol::modem::floor` are the
  production floor surface (`FloorFrame`, `FloorStream`, `FloorError`,
  `FLOOR_AIR_PROTOCOL_VERSION = 2`). The workspace takes a minor bump for
  the public addition.
<<<<<<< HEAD
- The experimental `coppa-bench` copy stays frozen for measurement
  continuity; production sweeps of the production path are not yet
  recorded (see `docs/LIMITATIONS.md`).
- The engine `CoppaCore`, the library Link and the daemon session
  controller do not construct or negotiate the floor yet; connecting on
  the floor, OFDM fallback and the −6 dB connect bar remain open, as does
  the Poor-channel sync share near the crossing.
=======
- The experimental `coppa-bench` copy retains its historical measurements.
  Production development sweeps cover batch and streaming reception, but
  do not pass sync share. Final-seed sweeps remain open
  (see `docs/LIMITATIONS.md` and BENCHMARKS, COP-190 section).
- The engine `CoppaCore` constructs separate batch and streaming floor
  receivers alongside OFDM. The library Link and daemon session controller
  do not negotiate or transmit on the floor yet. Connecting on the floor,
  OFDM fallback and the −6 dB connect bar remain open.
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b
- Version 2 spends one of the fourteen remaining version values; the next
  incompatible format allocates 3.
