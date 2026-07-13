# Coppa Waveform &amp; Protocol Conformance Specification

**Status:** Normative, wire-format version 1 (as built through Phase 3 of the
remediation roadmap, commit range ending at `f725f7c` on
`feature/field-readiness`).

## 0. Purpose, audience, and how to read this document

This document specifies the Coppa PHY waveform, frame header, forward error
correction (FEC), interleaving, and multi-codeword framing precisely enough
for an independent implementer to build a bit-compatible, interoperable
transmitter and receiver **without reading the Rust source**. It uses
RFC 2119-style **MUST** / **SHOULD** / **MAY** language.

Every normative numeric value or algorithm below is cited to the Rust
source symbol that defines it (`path/to/file.rs:SymbolName`), so a reviewer
can independently check each claim against the code. Sections or notes
marked **(Informative)** describe *this reference implementation's* internal
receiver strategy (e.g. its specific channel estimator or sync-detector
tuning); they are not required of a conformant independent implementation,
only the wire behavior they produce is.

Coppa is **not** RF/waveform-compatible with VARA or any other existing HF
modem; this spec describes only Coppa's own wire format.

### 0.1 Versioning and wire-format break history

This spec describes **wire-format version 1** — the format produced when
`CoppaTransceiver`/`CoppaModem` are constructed with `version: 1`, which is
what every current production caller uses
(`crates/coppa-engine/src/engine.rs:116,128,180,225`). The protocol version
number is also carried on the wire (`CoppaHeader::version`, §4) and keys the
preamble/probe pseudo-random sequences (§2, §3), so a receiver can in
principle distinguish version-1 frames from a future version's frames at the
preamble-correlation stage, but this codec does not implement multi-version
interop — a `CoppaTransceiver` instance decodes exactly the version it was
constructed with.

This codebase has already broken its own wire format twice before reaching
version 1's current shape; a future implementer should not assume any wire
compatibility across these boundaries:

- **Phase 1 waveform break** (carrier offset, Newman-phase preamble, TX
  conditioning, streaming sync, two-stage CFO) — see
  `docs/adr/003-phase1-waveform-break.md`.
- **NR BG2 LDPC break** (single shared BG2 mother code + circular-buffer
  rate matching replacing nine separate per-rate 802.11 QC-LDPC codes;
  speed level 10's nominal rate changed from 7/8 to 5/6) — see
  `docs/adr/005-nr-bg2-ldpc.md`.

Two further ADRs describe additions layered on top of the same wire format
(no further breaks): `docs/adr/007-multi-codeword-frames.md` (multi-codeword
framing + intra-frame cross-codeword interleaving, §9 below) and
`docs/adr/008-phase3-system-layer.md` (payload integrity, ARQ, IR-HARQ, rate
loop, SCO tracking — the system-layer behaviors this spec documents where
they touch the wire).

### 0.2 Two OFDM stacks — this spec covers only the canonical one

`crates/coppa-codec/src/ofdm/mod.rs:7-31` documents that this module tree
contains **two** parallel OFDM implementations: a "generic" pedagogical stack
(`OfdmProfile`, `OfdmModulator`/`OfdmDemodulator`,
`equalizer::LinearInterpolationEstimator`, and `frame::SignalField`/
`generate_sts`/`generate_lts`) that carries **no real traffic** and is not on
the engine's data path, and the canonical "Coppa stack"
(`CoppaProfile`, `coppa_modem::CoppaModem`, `pilots::CoppaPilotPattern`,
`frame::CoppaHeader`, `sync_detector::SyncDetector`) that the engine and
`CoppaTransceiver` actually use. **This spec describes only the Coppa
stack.** The generic stack's types (`OfdmProfile::HF_STANDARD`,
`SignalField`, Schmidl-Cox `generate_sts`/`generate_lts`, MCS_TABLE) are
mentioned nowhere below except this note, precisely so a reader does not
mistake them for the wire format.

---

## 1. Sample rate, FFT, cyclic prefix, symbol timing

All Coppa-stack parameters are carried per-profile in `CoppaProfile`
(`crates/coppa-codec/src/ofdm/mod.rs:154-176`). A conforming implementation
**MUST** support at least `hf_standard` (§1.1); support for the other
profiles below is **SHOULD**.

Every profile below uses `fft_size = 960` and `sample_rate = 48_000` Hz,
giving a **fixed subcarrier spacing of 50.0 Hz**
(`carrier_spacing_hz() = sample_rate / fft_size`,
`crates/coppa-codec/src/ofdm/mod.rs:298-301`).

### 1.1 Profile table

| Profile | `fft_size` | `sample_rate` | `cp_samples` | `data_carriers` | `pilot_carriers` | `phy_mode` | `bandwidth_id` | `carrier_offset` | symbol duration (incl. CP) |
|---|---|---|---|---|---|---|---|---|---|
| `hf_standard` | 960 | 48000 | 300 | 44 | 4 | 0 | 1 | 6 | 26.25 ms |
| `hf_robust` | 960 | 48000 | 300 | 36 | 12 | 0 | 3 | 6 | 26.25 ms |
| `hf_narrow` | 960 | 48000 | 300 | 8 | 2 | 0 | 0 | 6 | 26.25 ms |
| `hf_wide` | 960 | 48000 | 300 | 46 | 4 | 0 | 2 | 6 | 26.25 ms |
| `hf_standard_short_cp` | 960 | 48000 | 144 | 44 | 4 | 0 | 4 | 6 | 23.0 ms |
| `vhf_narrow` | 960 | 48000 | 60 | 44 | 4 | 1 | 1 | 6 | 21.25 ms |
| `vhf_wide` | 960 | 48000 | 60 | 104 | 8 | 1 | 2 | 6 | 21.25 ms |

Sources for each field: `crates/coppa-codec/src/ofdm/mod.rs:180-296`
(`CoppaProfile::hf_standard`, `hf_robust`, `hf_narrow`, `hf_wide`,
`hf_standard_short_cp`, `vhf_narrow`, `vhf_wide`). Symbol durations are
computed by `symbol_duration_ms() = (fft_size + cp_samples) / sample_rate *
1000` (`crates/coppa-codec/src/ofdm/mod.rs:308-311`) and independently
pinned by unit tests at `crates/coppa-codec/src/ofdm/mod.rs:824-826`
(26.25 ms), `:874-879` (23.0 ms), `:908-913` (21.25 ms).

Notes:

- `hf_robust` carries the same 48 total active carriers as `hf_standard`
  (36 data + 12 pilot vs. 44 data + 4 pilot) — a denser-pilot variant for
  frequency-selective HF multipath, not a different carrier count
  (`crates/coppa-codec/src/ofdm/mod.rs:193-208`, test at `:594-606`).
- `hf_standard_short_cp` is **identical** to `hf_standard` in every field
  except `cp_samples` (144 vs. 300) and `bandwidth_id` (4 vs. 1)
  (`crates/coppa-codec/src/ofdm/mod.rs:243-268`).
- `bandwidth_id` values are guaranteed pairwise-distinct **within each
  `phy_mode`** (test `crates/coppa-codec/src/ofdm/mod.rs:882-900`), but
  `vhf_narrow`'s `bandwidth_id` (1) collides numerically with
  `hf_standard`'s (1) — this is not a bug: profile identity on the wire is
  determined by the **pair** `(phy_mode, bandwidth_id)`, not `bandwidth_id`
  alone. See §1.3 for why this pair is not, in fact, used by the reference
  implementation to auto-select a profile.
- `carrier_offset` is `6` on **every** profile
  (`crates/coppa-codec/src/ofdm/mod.rs:189,206,220,239,266,280,294`).

### 1.2 Derived quantities

- Useful (data) symbol duration: `fft_size / sample_rate` = 20.0 ms for
  every profile above (`crates/coppa-codec/src/ofdm/mod.rs:314-316`).
- Symbols per second: `sample_rate / (fft_size + cp_samples)`
  (`crates/coppa-codec/src/ofdm/mod.rs:318-321`).
- First active FFT bin: `carrier_offset + 1` = bin **7** on every profile
  (`crates/coppa-codec/src/ofdm/mod.rs:323-326`).
- Active-carrier frequency band `[lo, hi]`:
  `lo = first_active_bin * carrier_spacing_hz`,
  `hi = (carrier_offset + total_active_carriers) * carrier_spacing_hz`
  (`crates/coppa-codec/src/ofdm/mod.rs:328-335`). For `hf_standard` this is
  **350–2700 Hz**; `hf_wide` is **350–2800 Hz**; `hf_narrow` is
  **350–800 Hz**; `vhf_wide` is **350–5900 Hz**. These HF bands are chosen
  to sit clear of a typical SSB radio's audio passband filters
  (`crates/coppa-codec/src/ofdm/mod.rs:170-174`), verified directly by test
  `crates/coppa-codec/src/ofdm/mod.rs:952-975` (lower edge ≥ 300 Hz, upper
  edge ≤ 2750 Hz for `hf_standard`/`hf_robust`/`hf_narrow`, ≤ 2850 Hz for
  `hf_wide`).

### 1.3 Profile selection is out-of-band, not signaled in-band

A conforming receiver **MUST** already know which `CoppaProfile` (i.e.
`fft_size`, `cp_samples`, carrier layout) the transmitter is using *before*
it can locate the preamble at all — the whole sync/demod pipeline is
parameterized by a `CoppaProfile` supplied at construction
(`CoppaModem::new`, `crates/coppa-codec/src/ofdm/coppa_modem.rs:293-318`;
`CoppaTransceiver::new`, `crates/coppa-protocol/src/modem/transceiver.rs:526-575`).
The header's `phy_mode`/`bandwidth` fields (§4) **MUST** be treated as
self-describing metadata only: this reference implementation does not
contain a `(phy_mode, bandwidth_id) → CoppaProfile` lookup that a receiver
consults to auto-select a profile at demodulation time. Profile agreement
between sender and receiver is an out-of-band (e.g. session-negotiation or
fixed-configuration) concern outside this spec's scope.

---

## 2. Carrier map and Hermitian packing

Coppa's OFDM signal is real-valued (transmitted as audio through an SSB
radio or sound card), produced by an IFFT with **Hermitian symmetry**: every
active positive-frequency bin `k` has its complex conjugate placed at bin
`fft_size - k`.

- Active carrier `i` (`i` in `0..total_active_carriers`) maps to FFT bin
  `first_active_bin() + i = carrier_offset + 1 + i`
  (`crates/coppa-codec/src/ofdm/mod.rs:323-326`,
  `crates/coppa-codec/src/ofdm/coppa_modem.rs:414-432` `build_ofdm_symbol`).
- For each active carrier, `freq[bin] = value` and
  `freq[fft_size - bin] = conj(value)`, **except** the Nyquist bin
  (`bin == fft_size/2`), which **MUST** carry only a real value
  (`crates/coppa-codec/src/ofdm/coppa_modem.rs:423-432`).
- A conforming implementation **MUST** keep `carrier_offset +
  total_active_carriers < fft_size / 2` (checked by `debug_assert!` at
  `crates/coppa-codec/src/ofdm/coppa_modem.rs:418-421`); every profile in
  §1.1 satisfies this with margin.
- The time-domain OFDM symbol is `IFFT(freq)`, taking the real part, with a
  cyclic prefix of `cp_samples` prepended (the *last* `cp_samples` samples
  of the IFFT output, copied to the front)
  (`crates/coppa-codec/src/ofdm/coppa_modem.rs:434-441`).

Within the active carrier range, positions are split between **pilot**
carriers (known values, for channel estimation) and **data** carriers
(payload/header bits); see §3.3 for the pilot placement pattern.

---

## 3. Preamble, probe symbol, and pilot pattern

Every Coppa frame begins with three fixed-content OFDM-symbol-lengths
before the header: a 2-symbol Newman-phase preamble, then one full-comb PN
probe symbol. `data_start = timing_offset + 3 * symbol_len` where
`symbol_len = fft_size + cp_samples`
(`crates/coppa-codec/src/ofdm/coppa_modem.rs:621`), confirming this 3-symbol
fixed prefix is structural, not incidental.

### 3.1 Preamble: Newman-phase comb, two identical symbols

The preamble occupies the **even-indexed** FFT bins within the profile's
active band:

```
preamble_comb_bins(profile) = { b in first_active_bin()..=(carrier_offset + total_active_carriers) : b % 2 == 0 }
```

(`crates/coppa-codec/src/ofdm/sync.rs:57-61`). For `hf_standard` this is
bins `{8, 10, 12, ..., 54}` — 24 tones. Even-only placement makes the
symbol body periodic with period `fft_size/2`, which is the property the
Schmidl-Cox two-identical-halves structure and the coarse CFO lag both rely
on (`crates/coppa-codec/src/ofdm/sync.rs:53-56`).

Each comb tone `k` (0-indexed within the comb, `k_tones` = comb length) gets
phase:

```
phi_k = pi * ((k + rotation) mod k_tones)^2 / k_tones
```

where `rotation = version` (the protocol version number)
(`crates/coppa-codec/src/ofdm/sync.rs:63-73`, `newman_phases`). This is a
**Newman phase sequence**, chosen for near-minimal peak-to-average power
ratio for an equal-amplitude comb; the `rotation` cyclically shifts the
phase assignment to **key the preamble by protocol version** without
changing its envelope class — this is the "version rotation for wire-format
identification" mechanism. Test
`crates/coppa-codec/src/ofdm/sync.rs:422-433`
(`preamble_versions_are_distinguishable`) confirms version-1 vs. version-2
combs decorrelate (normalized dot product < 0.5).

Construction (`generate_coppa_preamble`,
`crates/coppa-codec/src/ofdm/sync.rs:84-116`):

1. Place `v = (cos(phi_k), sin(phi_k))` at bin `bin_k` and `conj(v)` at bin
   `fft_size - bin_k` for every comb bin, IFFT.
2. Prepend the cyclic prefix (last `cp_samples` samples), producing one
   `symbol_len`-sample symbol.
3. **Unit-RMS normalize** this one symbol (divide by its own RMS).
4. Emit this normalized symbol **twice** (concatenated) — the 2-symbol,
   two-identical-halves preamble, total length `2 * symbol_len`.

Measured PAPR for `hf_standard`'s 24-tone comb is ~4.9–5.8 dB across
versions 1–4 (`crates/coppa-codec/src/ofdm/sync.rs:398-414`) — note this is
higher than the ~3 dB asymptotic Newman figure that only holds for large
tone counts; the code comment explicitly flags this as a measured,
independently-reverified correction to the design-doc's asymptotic claim.

The PN sequence for the probe symbol (§3.2) uses a **7-bit LFSR**,
polynomial `x^7 + x + 1` (taps at bits 6 and 0), seeded with
`(version + 1) & 0x7F` (never zero), producing `480` (= `fft_size / 2`)
values of `+1.0`/`-1.0`
(`coppa_pn_sequence`, `crates/coppa-codec/src/ofdm/sync.rs:21-51`).

### 3.2 Probe symbol: full-comb PN

Immediately after the 2-symbol preamble, one full OFDM symbol (**not**
just even bins — **every** active carrier, `total_active_carriers` of them)
carries the same version-keyed PN sequence (`coppa_pn_sequence(version)`),
indexed modulo the PN sequence's length, mapped to real BPSK values:

```
probe_carriers[i] = Complex(pn[i mod pn.len()], 0.0)   for i in 0..total_active_carriers
```

(`crates/coppa-codec/src/ofdm/coppa_modem.rs:482-490`). This full-comb probe
serves as a channel-estimation reference the delay-domain estimator
consumes for its per-frame bulk-delay calibration (§ implementation note
below); it is skipped by decode logic that doesn't need it. It replaced an
earlier all-ones symbol whose impulse-like time-domain PAPR (~19.8 dB) was
being clipped into spectral splatter — see the comment at
`crates/coppa-codec/src/ofdm/coppa_modem.rs:482-485`.

**(Informative)** The reference receiver uses this probe symbol to
calibrate a fixed per-frame bulk-delay bias
(`CoppaModem::measure_bulk_bias`/`probe_calibration`,
`crates/coppa-codec/src/ofdm/coppa_modem.rs:320-402`) before payload
equalization. This is a receiver-side estimation strategy, not a wire
requirement — an independent receiver implementation is free to use the
probe symbol differently as long as it correctly demodulates the same
transmitted bits.

### 3.3 Pilot pattern: even/odd alternation, all-`+1` value

Pilots use `CoppaPilotPattern` (`crates/coppa-codec/src/ofdm/pilots.rs:121-227`),
constructed once per `CoppaModem` from `(total_active_carriers,
pilot_carriers)`. **Pilot values are always the constant `Complex(1.0,
0.0)`** — there is no PN scrambling of pilot *values* (only the preamble
and probe symbols use PN/Newman phasing; ordinary payload/header pilots do
not). This has not changed since before Phase 2; the delay-domain estimator
and Kalman tracker work (Phase 2) changed *how pilots are used at RX*, not
their transmitted values.

Placement alternates by **OFDM symbol index parity** (symbol 0, 2, 4, ...
use one pattern; symbols 1, 3, 5, ... use a second, offset pattern), giving
denser effective frequency coverage when pooled across adjacent symbols:

```
spacing       = total_carriers / num_pilots
half_spacing  = spacing / 2
even_indices  = { min(i * spacing, total_carriers - 1) : i in 0..num_pilots }
odd_indices   = { min(i * spacing + half_spacing, total_carriers - 1) : i in 0..num_pilots }
```

(`crates/coppa-codec/src/ofdm/pilots.rs:133-157`). `pilot_indices(symbol_num)`
returns `even_indices` if `symbol_num % 2 == 0`, else `odd_indices`
(`crates/coppa-codec/src/ofdm/pilots.rs:159-166`), confirmed periodic with
period 2 by test `crates/coppa-codec/src/ofdm/pilots.rs:287-295`. For
`hf_standard` (48 total active carriers, 4 pilots): `spacing = 12`,
`even_indices = {0, 12, 24, 36}`, `odd_indices = {6, 18, 30, 42}`.

All non-pilot indices in the active-carrier range for a given symbol are
data carriers (`data_indices`, `crates/coppa-codec/src/ofdm/pilots.rs:169-174`).

---

## 4. Frame header

### 4.1 Symbol ordering

After the 3-symbol preamble+probe prefix (§3), the frame carries, in order:
protected header OFDM symbols, then payload OFDM symbols
(`modulate_mapped`, `crates/coppa-codec/src/ofdm/coppa_modem.rs:470-558`).
Header and payload data are both mapped through the *same* pilot-inserting
`build_ofdm_symbol` path, using the profile's alternating pilot pattern
(§3.3) at the correct running symbol index (header symbols occupy indices
`0..num_header_syms`; payload symbols continue from `num_header_syms`).

### 4.2 48-bit header field layout

`CoppaHeader` (`crates/coppa-codec/src/ofdm/frame.rs:239-276`) packs into
exactly 6 bytes, **MSB-first within each field and each byte**
(`to_bytes`/`from_bytes`, `crates/coppa-codec/src/ofdm/frame.rs:280-317`):

```
byte 0: [version:4]      [phy_mode:4]
byte 1: [frame_type:4]   [bandwidth:4]
byte 2: [fec_type:4]     [speed_level:4]
byte 3: [seq_num:8]
byte 4: [payload_len bits 11..4]
byte 5: [payload_len bits 3..0] [codewords-1:4]
```

Field semantics:

- `version` (4 bits): protocol version, keys the preamble/probe PN (§3).
- `phy_mode` (4 bits): `0` = HF/SSB, `1` = VHF
  (`crates/coppa-codec/src/ofdm/mod.rs:166`). Self-describing only (§1.3).
- `frame_type` (4 bits): `CoppaFrameType` enum — `Data=0, Ack=1, Nak=2,
  Connect=3, ConnectAck=4, Disconnect=5, Beacon=6`
  (`crates/coppa-codec/src/ofdm/frame.rs:186-209`). Values `7..=15` are
  invalid; `from_bytes`/`from_bits` return `None` for them
  (`CoppaFrameType::from_u8`, same file, `:197-208`).
- `bandwidth` (4 bits): `bandwidth_id` (§1.1). Self-describing only (§1.3).
- `fec_type` (4 bits): low 2 bits (`fec_type & 0x03`, `RV_MASK`,
  `crates/coppa-protocol/src/modem/transceiver.rs:243`) are the IR-HARQ
  redundancy version `rv` (0–3, §5.3); high 2 bits are reserved, always `0`
  in this codec (`crates/coppa-codec/src/ofdm/frame.rs:249-258`).
- `speed_level` (4 bits): wire-encoded MCS/speed level, 1–10 (0, 8, 11–15
  invalid/reserved — level 8 is reserved for a future 32-QAM mode; see §6).
- `seq_num` (8 bits): sequence number, 0–255.
- `payload_len` (12 bits, max 4095): **total** application-payload length in
  bytes, summed across **all** codewords for a multi-codeword frame (§9) —
  not any single codeword's share
  (`crates/coppa-codec/src/ofdm/frame.rs:263-269`).
- `codewords` (4-bit wire nibble, decoded value `1..=16`): the wire nibble
  stores `codewords - 1`, so `codewords == 1` (the overwhelmingly common,
  pre-multi-codeword-era case) encodes to nibble `0` — byte-for-byte
  identical to the pre-Task-5 wire format's `reserved: 0`
  (`crates/coppa-codec/src/ofdm/frame.rs:220-238,283`). Only `1..=8` are
  produced/accepted by `CoppaTransceiver` today (`MAX_CODEWORDS = 8`,
  `crates/coppa-protocol/src/modem/transceiver.rs:205`); `9..=16` round-trip
  on the wire but are rejected by `transmit`
  (`TransmitError::TooManyCodewords`,
  `crates/coppa-protocol/src/modem/transceiver.rs:654-660`) — see §9.

Round-trip and boundary behavior verified by tests at
`crates/coppa-codec/src/ofdm/frame.rs:427-588` (all header fields, all
frame types, max payload_len=4095, full codewords range including the
headroom values 9 and 16).

### 4.3 Protected-header FEC framing

The 48 raw header bits are **never** sent unprotected. `header_fec`
(`crates/coppa-codec/src/ofdm/header_fec.rs`) wraps them:

```
48 header bits + 16-bit CRC + 8-bit zero pad = 72 info bits
  -> 6 x Golay(24,12) codewords = 144 coded bits
  -> stride-5 bit interleave
  -> BPSK (bit 0 -> +1, bit 1 -> -1)
```

(`crates/coppa-codec/src/ofdm/header_fec.rs:6-33,76-107`). Constants:
`PROTECTED_HEADER_CODED_BITS = 144`
(`crates/coppa-codec/src/ofdm/header_fec.rs:26`), `INFO_BITS = 72`, `N_WORDS
= 6`, `INTERLEAVE_STRIDE = 5` (`:28-33`).

- **CRC-16**: computed over the raw 6 header bytes (`hb = header.to_bytes()`)
  using `CRC_16_IBM_SDLC` (the `crc` crate's named constant for that
  polynomial family), appended MSB-first as 16 info bits
  (`crates/coppa-codec/src/ofdm/header_fec.rs:23,79,88-90`).
- **Zero pad**: 8 bits, always `0`, appended after the CRC to reach 72 info
  bits total (`crates/coppa-codec/src/ofdm/header_fec.rs:91`). A decoder
  **MUST** reject (drop) any candidate whose decoded pad bits are nonzero
  (`crates/coppa-codec/src/ofdm/header_fec.rs:117-120`).
- **Golay(24,12) generator**: systematic `G = [I_12 | A]`, with `A`'s 12
  row masks (12-bit, MSB = column 0) given by

  ```
  A_ROWS = [0x7FF, 0xEE2, 0xB71, 0xDB8, 0xADC, 0x96E,
            0x8B7, 0xC5B, 0xE2D, 0xF16, 0xB8B, 0xDC5]
  ```

  (`crates/coppa-codec/src/ofdm/golay.rs:13-15`). `golay24_encode(info)`
  builds the 24-bit codeword as `[info(12 bits) | parity(12 bits)]`, info in
  bit positions 23..12
  (`crates/coppa-codec/src/ofdm/golay.rs:22-32`). Each of the 6 12-bit info
  chunks is packed MSB-first from the 72 info bits
  (`crates/coppa-codec/src/ofdm/header_fec.rs:95-104`). The minimum distance
  is 8, correcting up to 3 errors per 24-bit word (verified offline per the
  module doc, `crates/coppa-codec/src/ofdm/golay.rs:5-7`; correction-radius
  test at `crates/coppa-codec/src/ofdm/header_fec.rs:262-271`).
- **Stride-5 interleave**: `interleave(coded)[(i * 5) mod 144] = coded[i]`
  (coded bit `i` is placed at output position `(i * 5) mod 144`) is applied
  to the concatenated 144 coded bits before BPSK mapping
  (`crates/coppa-codec/src/ofdm/header_fec.rs:35-41`), a bijection since
  `gcd(5, 144) = 1` (round-trip test at `:254-259`). This spreads each
  Golay word's 24 bits across the header's OFDM symbols so a persistent
  single-carrier or single-symbol fading null does not concentrate errors
  in one word beyond its 3-error correction budget.
- **BPSK mapping**: coded bit `0` → `Complex(+1, 0)`, bit `1` → `Complex(-1,
  0)` (`crates/coppa-codec/src/ofdm/coppa_modem.rs:494-503`) — the same
  convention as `BpskMapper` (§6.1).

A receiver **MUST** implement (or an equivalent of) the soft decoder used on
the live RX path, `decode_header_soft`
(`crates/coppa-codec/src/ofdm/header_fec.rs:164-223`): deinterleave 144
received LLRs, soft-ML-decode each of the 6 Golay words to its 2
best-scoring candidate info words (`golay24_decode_soft`,
`crates/coppa-codec/src/ofdm/golay.rs:121`), then search combinations (all
best-guesses first, then each single word flipped to its runner-up, then
all pairs flipped — at most `1+6+15=22` CRC-16 checks) for the first
combination whose zero-pad and CRC-16 both pass. This is a receiver
strategy choice (an independent implementation could instead attempt only
the hard-decision `decode_header`, `crates/coppa-codec/src/ofdm/header_fec.rs:143-162`,
at a real robustness cost); the **wire format** (Golay generator, CRC
polynomial, pad, interleave stride) in this section is what is normative.

`num_header_syms = PROTECTED_HEADER_CODED_BITS.div_ceil(data_carriers_per_symbol)`
(`crates/coppa-codec/src/ofdm/coppa_modem.rs:637`); for `hf_standard` (44
data carriers/symbol) this is `144.div_ceil(44) = 4` OFDM symbols. The last
header symbol is padded with `Complex(1,0)` (BPSK-zero) past the 144 real
coded bits (`crates/coppa-codec/src/ofdm/coppa_modem.rs:505-513`).

---

## 5. Forward error correction (payload)

### 5.1 Mother code: 3GPP NR BG2, Zc = 176

Every speed level's payload FEC is **one shared** 5G-NR-style LDPC base
graph 2 (BG2) mother code, auto-transcribed from **3GPP TS 38.212 §5.3.2
Table 5.3.2-3**, lifting-size family `i_LS = 5` fixed at `Zc = 176`
(`crates/coppa-protocol/src/fec/ldpc/nr_bg2.rs:1-24`; generator provenance
documented in `tools/gen_nr_bg2/src/main.rs`). Key constants:

| Constant | Value | Source |
|---|---|---|
| `ZC` (lifting factor) | 176 | `nr_bg2.rs:14` |
| `BASE_ROWS` | 42 | `nr_bg2.rs:16` |
| `BASE_COLS` | 52 | `nr_bg2.rs:18` |
| `KB` (info columns) | 10 | `nr_bg2.rs:20` |
| `CORE_PARITY_COLS` | 4 | `nr_bg2.rs:22` |
| `PUNCTURED_INFO_COLS` | 2 | `nr_bg2.rs:24` |
| Non-zero base-graph entries | 197 | `nr_bg2.rs:26-79` |
| `NrLdpc::INFO_LEN` (`KB*ZC`) | 1760 | `ldpc/mod.rs:96` |
| `NrLdpc::MOTHER_LEN` (`(BASE_COLS-PUNCTURED_INFO_COLS)*ZC`) | 8800 | `ldpc/mod.rs:98` |

`NrLdpc::encode(info: [u8; 1760]) -> [u8; 8800]` produces
`[info[2*ZC..], parity]`: the leading `2*ZC = 352` systematic bits are
**punctured** (never transmitted, standard NR practice), followed by all
parity bits (`crates/coppa-protocol/src/fec/ldpc/mod.rs:100-110`).

**Note on a superseded legacy codec:** `crates/coppa-protocol/src/fec/ldpc/codes.rs`
defines a *different*, older set of six per-rate QC-LDPC codes (IEEE
802.11-2012 Annex F base matrices, `Z = 81`, `N = 1944`) and a `CodeRate`
enum. Per `crates/coppa-protocol/src/fec/ldpc/mod.rs:67-70`: "kept for
reference/back-compat but **no longer used by `CoppaTransceiver`**." A
conforming implementation **MUST NOT** use this legacy codec's base
matrices — only the NR BG2 mother code above is on the wire. See §11 for a
labeling inconsistency this legacy enum causes.

### 5.2 Shortening + rate matching (per-speed-level rate realization)

Every speed level uses the **same** mother code; its nominal code rate is
realized by *shortening*: only the first `k_used` of the mother code's 1760
systematic info bits actually carry payload+padding; the remaining
`1760 - k_used` are known zero-pad, never transmitted, and reconstructed
(pinned) at RX (§5.4). `k_used_for_level`
(`crates/coppa-protocol/src/modem/speed_levels.rs:61-75`):

| Wire level | Modulation | `k_used` | Nominal rate | Notes |
|---|---|---|---|---|
| 1 | BPSK | 486 | 1/4 | |
| 2 | BPSK | 972 | 1/2 | |
| 3 | QPSK | 972 | 1/2 | |
| 4 | QPSK | 1458 | 3/4 | |
| 5 | 8PSK | 1296 | 2/3 | |
| 6 | 16QAM | 972 | 1/2 | |
| 7 | 16QAM | 1458 | 3/4 | |
| 8 | (32QAM) | — | — | **reserved**, `k_used_for_level(8) = None` |
| 9 | 64QAM | 1296 | 2/3 | |
| 10 | 64QAM | 1620 | **5/6** | wire-format-breaking change from a pre-Task-4 7/8 (1701) — see §0.1 |

(`k_used_for_level` match arms, `crates/coppa-protocol/src/modem/speed_levels.rs:62-74`;
audited-ladder round-trip test at `crates/coppa-protocol/src/fec/ldpc/mod.rs:245-255`.)

`CODED_BLOCK_LEN = 1944` is the fixed number of coded bits selected out of
the rate-matching circular buffer for **every** speed level, regardless of
`k_used` (`crates/coppa-protocol/src/modem/transceiver.rs:173`; matches the
legacy codec's `CODED_BITS` value by coincidence of both independently being
1944, not by sharing implementation).

Rate matching follows **3GPP TS 38.212 §5.4.2**'s circular-buffer procedure
(`crates/coppa-protocol/src/fec/ldpc/rate_match.rs:1-29`):

```
buffer(k_used) = mother_info[0 .. k_used - 2*ZC]  ++  mother_parity[..]
```

i.e. the transmitted (non-punctured, non-shortened) info prefix followed by
*all* parity bits — the buffer is never further truncated
(`rate_match.rs:16-24`). `E = 1944` coded bits are then read **circularly**
starting at offset `k0(rv)`:

```
k0(rv) = floor( raw(rv) / ZC ) * ZC,  where raw(rv) = 0, buf_len/4, buf_len/2, 3*buf_len/4  for rv = 0,1,2,3
```

(`k0_offset`, `crates/coppa-protocol/src/fec/ldpc/rate_match.rs:78-87`) —
`k0` is always rounded down to a `Zc`-multiple boundary, since a QC-LDPC
circular-buffer read must start on a lifted-block boundary. `rate_match`
(`:95-100`) and its LLR-domain mirror `rate_match_llr` (`:115-120`) both
select `buf[(k0+i) mod buf_len]` for `i in 0..E`.

### 5.3 Redundancy versions and IR-HARQ

`fec_type & RV_MASK` (§4.2) carries `rv in 0..=3`. A fresh (non-retransmitted)
frame **MUST** use `rv = 0` (`k0 = 0`, i.e. the buffer's start). On
retransmission, the sender **SHOULD** cycle RV in the order
`rv_for_attempt(attempt) = [0, 2, 3, 1][attempt mod 4]`
(`crates/coppa-protocol/src/arq.rs:164-167`) — standard LTE/5G-NR IR-HARQ
order (RV2 first, maximizing new parity coverage since RV0/RV2 sample very
different regions of the buffer), cycling back to RV0 (Chase-combining) on
the 4th retransmission. A receiver combines soft LLRs across retransmissions
of the same `seq` by rate-dematching each transmission's LLRs at its own
`rv` into the shared mother-length buffer and **summing** overlapping
positions (`rate_dematch`'s doc, `crates/coppa-protocol/src/fec/ldpc/rate_match.rs:122-140`).

### 5.4 Scrambling and known-pad LLR pinning

Before LDPC encoding, the 1760-bit info block (payload + CRC-32 trailer +
zero padding out to `k_used`, further zero-padded to 1760) is **XORed with
a PRBS scrambler**: polynomial `x^15 + x^14 + 1` (DVB-S2-style), fixed seed
`0x4A80`, self-inverse (`crates/coppa-protocol/src/fec/scrambler.rs:1-16`).
The keystream bit at index `i` depends only on the LFSR's own running state
(not on the data being scrambled), which is what lets a receiver compute
the exact scrambled value of any all-zero padding region without knowing
the real payload (`prbs_bits`, `crates/coppa-protocol/src/fec/scrambler.rs:18-33`).

A receiver **SHOULD** pin known-zero padding LLRs to a high-confidence value
before LDPC decoding rather than trusting the (noisy or entirely absent)
channel observation there: `pin_known_pad`
(`crates/coppa-protocol/src/fec/ldpc/mod.rs:186-210`) sets every info-bit
position in `payload_bits..KB*ZC` (both the shortened-but-transmitted pad
`payload_bits..k_used` and the never-transmitted shortened tail
`k_used..KB*ZC`) to `±pin` according to the PRBS keystream's expected value
at that index, leaving genuine payload bits inside the always-punctured
leading `2*ZC` region untouched.

### 5.5 Payload integrity: per-codeword CRC-32

Each codeword's payload chunk gets its own 4-byte trailer:
`CRC_32_ISO_HDLC` checksum, appended **big-endian**
(`checksum.to_be_bytes()`), before LDPC info-bit packing
(`crates/coppa-protocol/src/modem/transceiver.rs:4,185,689-692`).
`PAYLOAD_CRC_LEN = 4` (`transceiver.rs:192`). A receiver **MUST** verify
this CRC-32 after LDPC decoding and descrambling and **MUST** reject the
codeword's payload on mismatch.

### 5.6 Non-normative: reference decoder algorithm

**(Informative)** The reference LDPC decoder is a layered normalized
min-sum belief-propagation decoder with scale `NR_DEFAULT_SCALE = 0.75` and
iteration cap `NR_DEFAULT_MAX_ITERATIONS = 30`
(`crates/coppa-protocol/src/fec/ldpc/decoder.rs:365,371,414`). This is a
receiver implementation choice, not a wire requirement — any decoder that
correctly decodes NR BG2 codewords built per §5.1–§5.2 is conformant. (Do
not confuse this `0.75` with the *legacy* per-rate codec's unrelated
`DEFAULT_SCALE = 0.8` / `DEFAULT_MAX_ITERATIONS = 50`,
`crates/coppa-protocol/src/fec/ldpc/decoder.rs:32,35` — that decoder
belongs to the superseded codec of §5.1 and is not used by
`CoppaTransceiver`.)

---

## 6. Constellation mapping and speed-level table

`SPEED_LEVELS` (`crates/coppa-codec/src/ofdm/coppa_modem.rs:37-101`) and
`speed_level_components` (`crates/coppa-protocol/src/modem/speed_levels.rs:6-28`)
together define, per wire level:

| Level | Mapper | `bits_per_symbol` | PAPR target (dB) |
|---|---|---|---|
| 1 | BPSK | 1 | 6.0 |
| 2 | BPSK | 1 | 6.0 |
| 3 | QPSK | 2 | 7.0 |
| 4 | QPSK | 2 | 7.0 |
| 5 | 8PSK | 3 | 8.0 |
| 6 | 16QAM | 4 | 9.5 |
| 7 | 16QAM | 4 | 11.0 |
| 8 | — | — | — (reserved, 32-QAM) |
| 9 | 64QAM | 6 | 11.0 |
| 10 | 64QAM | 6 | 14.0 |

(`SPEED_LEVELS` array, `crates/coppa-codec/src/ofdm/coppa_modem.rs:37-101`;
mapper selection, `crates/coppa-protocol/src/modem/speed_levels.rs:15-27`).
PAPR targets are consumed by `papr_clip` (§7) at TX.

### 6.1 Per-mapper bit conventions (`coppa-codec`)

- **BPSK** (`BpskMapper`, `crates/coppa-codec/src/bpsk.rs:13-40`): bit `0` →
  `+1`, bit `1` → `-1`.
- **QPSK** (`QpskMapper`, `crates/coppa-codec/src/qpsk.rs:31-49`): 2 bits
  `(b0, b1)` map to `SCALE*(1-2*b1, 1-2*b0)` with
  `SCALE = 1/sqrt(2)` — `b0` sets the imaginary (Q) sign, `b1` the real (I)
  sign. Constellation table:
  `00→(S,S), 01→(-S,S), 10→(S,-S), 11→(-S,-S)`
  (`crates/coppa-codec/src/qpsk.rs:35-39`).
- **8PSK** (`Psk8Mapper`, `crates/coppa-codec/src/psk8.rs:1-40`): 3 bits
  Gray-coded via `GRAY_ORDER = [0,1,3,2,6,7,5,4]`
  (`crates/coppa-codec/src/psk8.rs:13`) into one of 8 equally-spaced unit
  circle points at angle `2*pi*index/8`.
- **16QAM** (`Qam16Mapper`, `crates/coppa-codec/src/qam16.rs:1-40`): I axis
  uses bits `[b0,b1]`, Q axis uses bits `[b2,b3]`. Per-axis Gray mapping
  `idx=(b_msb<<1)|b_lsb` into `LEVEL=[3,1,-1,-3]` (00→+3, 01→+1, 11→-1,
  10→-3), normalized by `NORM = 1/sqrt(10)` (`crates/coppa-codec/src/qam16.rs:15-25`).
- **64QAM** (`Qam64Mapper`, `crates/coppa-codec/src/qam64.rs:1-45`): I axis
  uses bits `[b0,b1,b2]`, Q axis uses `[b3,b4,b5]`. 3-bit Gray table
  `GRAY_TO_IDX=[0,1,3,2,7,6,4,5]` into `LEVEL=[7,5,3,1,-1,-3,-5,-7]`,
  normalized by `NORM = 1/sqrt(42)` (`crates/coppa-codec/src/qam64.rs:14-27`).

---

## 7. Interleavers

Coppa uses two interleaving stages, layered:

### 7.1 Intra-codeword block interleaver (`BlockInterleaver`)

For every codeword, the `CODED_BLOCK_LEN = 1944` coded bits are written
row-wise across `carriers = profile.data_carriers` columns and read
column-wise (spreading adjacent coded bits across both time and frequency):

```
rows = ceil(block_size / carriers)
write: grid[row * carriers + col] = bits[row * carriers + col]   (row-major fill)
read:  output = [ grid[row*carriers+col] for col in 0..carriers for row in 0..rows if row*carriers+col < block_size ]
```

(`BlockInterleaver::new`/`interleave`,
`crates/coppa-codec/src/ofdm/interleaver.rs:11-37`). **Pad cells (grid
positions ≥ `block_size`) are skipped, never emitted** — this is a
deliberate, tested fix for a historic puncture bug where 35 of 1944 bits
were silently dropped for the `1944/44`-carrier case (regression tests at
`crates/coppa-codec/src/ofdm/interleaver.rs:87-125`). A soft-LLR mirror,
`interleave_soft`, applies the identical index permutation for `f32` values
(`:46-62`).

### 7.2 Intra-frame cross-codeword interleaver (`CrossFrameInterleaver`, reused)

For multi-codeword frames (§9) at speed level ≥ 5, an additional
interleaving stage spreads each codeword's 1944 coded bits evenly across
all `codewords` time-slots **within the same frame**, so a single faded
region damages only `1/codewords` of every codeword rather than
concentrating damage in one codeword.

This reuses `CrossFrameInterleaver`
(`crates/coppa-codec/src/ofdm/cross_frame_interleaver.rs`) — despite its
name and module doc describing spreading "across `N` **frames**"
(`:1-5`), `CoppaTransceiver::transmit` invokes it with `codewords` playing
the role of "`N` frame-blocks" and 1 frame's `codewords` codewords playing
the role of "`N` codewords", i.e. the *same permutation math* is
reinterpreted as "N codeword time-slots within one frame" instead of its
originally-described cross-frame use
(`crates/coppa-protocol/src/modem/transceiver.rs:709-727`, explicit comment
at `:710-717`). See §9 for the gating condition.

For `num_frames = N` (here, `codewords`) and `coded_bits_per_codeword = C`
(= 1944), with `Cn = C / N` (even stripe size):

```
even = Cn * N
for input = k*C + i  (codeword k, position i):
  if i < even:
    g = i / Cn; b = i % Cn; f = (k + g) mod N
    output = f*C + (k*Cn + b)
  else:                                  # remainder region, only if N does not divide C
    r = i - even; f = (k + r) mod N
    output = f*C + even + r
```

(`CrossFrameInterleaver::new`, `crates/coppa-codec/src/ofdm/cross_frame_interleaver.rs:17-51`).
This is a proven bijection (round-trip tests at
`crates/coppa-codec/src/ofdm/cross_frame_interleaver.rs:78-131`), including
the non-exact-division remainder branch.

### 7.3 Deleted component: no protocol-side generic interleaver

CLAUDE.md's Known Limitations states the old `coppa-protocol::fec::interleaver`
was deleted as dead code in Phase 3 Task 9. This spec confirms that by
direct inspection: `crates/coppa-protocol/src/fec/` contains only
`convolutional.rs`, `ldpc/`, `mod.rs`, and `scrambler.rs` — **no**
`interleaver.rs`. The interleaving described in §7.1–§7.2 lives entirely in
`coppa-codec`, not `coppa-protocol`.

---

## 8. Header FEC A_ROWS / CRC / interleave — see §4.3

(Kept as a single combined section above rather than duplicated, since
header FEC and payload FEC use unrelated mechanisms — Golay(24,12) +
CRC-16 for the header vs. NR BG2 LDPC + CRC-32 for the payload.)

---

## 9. Multi-codeword framing

A single frame **MAY** carry `codewords` (1–8 in production; see §4.2 for
the full 1–16 wire-representable range) independent LDPC codewords
back-to-back in its payload, each with its own CRC-32 trailer (§5.5).
`MAX_CODEWORDS = 8` (`crates/coppa-protocol/src/modem/transceiver.rs:205`);
`CoppaTransceiver::transmit` **MUST** reject `header.codewords >
MAX_CODEWORDS` with `TransmitError::TooManyCodewords`
(`crates/coppa-protocol/src/modem/transceiver.rs:654-660`).

### 9.1 Payload splitting

`header.payload_len` (§4.2) is the **total** payload length across all
codewords. `split_payload_across_codewords(total_len, codewords)` computes
each codeword's `(start, len)` byte range:

```
base      = total_len / codewords          (integer division)
remainder = total_len % codewords
chunk[k].len = base + 1  if k < remainder else base
```

(`crates/coppa-protocol/src/modem/transceiver.rs:508-523`) — the `+1` goes
to the **first** `remainder` chunks, guaranteeing (given the caller's
oversize check) no single chunk ever exceeds
`max_payload_for_level(level)`. TX (`transmit`,
`crates/coppa-protocol/src/modem/transceiver.rs:676`) and RX
(`receive_core`, `crates/coppa-protocol/src/modem/transceiver.rs:1173`)
**MUST** use this identical algorithm — they must never diverge, or a
receiver would slice a sender's codewords at the wrong byte boundaries.

`max_multi_payload_for_level(level, codewords) = codewords *
max_payload_for_level(level)`, where `max_payload_for_level(level) =
k_used_for_level(level)/8 - PAYLOAD_CRC_LEN` (integer division)
(`crates/coppa-protocol/src/modem/speed_levels.rs:87-102`).

### 9.2 Per-frame pipeline

For each codeword `k` in order: append CRC-32 trailer → pack into a
1760-bit NR BG2 info block (payload+CRC bits, zero-padded to `k_used`, then
to 1760) → scramble → LDPC-encode → rate-match to 1944 coded bits at the
frame's `rv` (§5.2–§5.4)
(`crates/coppa-protocol/src/modem/transceiver.rs:685-707`). All `codewords`
codewords' 1944-bit blocks are concatenated codeword-major
(`all_coded`, length `codewords * 1944`).

### 9.3 Cross-codeword interleave gate

`CROSS_CODEWORD_INTERLEAVE_MIN_LEVEL = 5`
(`crates/coppa-protocol/src/modem/transceiver.rs:26`). The §7.2 interleave
is applied **iff** `codewords > 1 && speed_level >= 5`
(`cross_codeword_interleave_enabled`,
`crates/coppa-protocol/src/modem/transceiver.rs:607-609,722`); otherwise
each codeword's 1944 bits are transmitted as their own contiguous
time-slot, unpermuted. A single-codeword frame always skips this stage.

### 9.4 Per-slot mapping

Each of the `codewords` 1944-bit time-slots (post-interleave-gate, §9.3) is
independently passed through the §7.1 `BlockInterleaver` and then the
speed level's constellation mapper (§6); the resulting per-slot symbol
streams are concatenated in slot order into one flat payload-symbol stream
for `modulate_mapped`, which has no notion of codewords — it just packs
symbols into consecutive OFDM symbols
(`crates/coppa-protocol/src/modem/transceiver.rs:729-743`).

### 9.5 Known scope cuts (not implemented)

Per CLAUDE.md's Known Limitations and `docs/adr/007-multi-codeword-frames.md`
(decisions 4–5): ACK addressing, turbo re-estimation, and persistent IR-HARQ
LLR combining are **not** extended to the per-codeword level. A
multi-codeword frame is retransmitted, if at all, as a whole (same `seq`,
cycling `rv` via the mechanism in §5.3) rather than per
`(seq, codeword-index)`; turbo re-estimation and IR-HARQ combining are only
active for `codewords <= 1`, taking the exact pre-multi-codeword decode
path otherwise.

---

## 10. TX conditioning

After OFDM-modulating the preamble + probe + header + payload symbol
sequence (§3–§6, §9), `modulate_mapped`
(`crates/coppa-codec/src/ofdm/coppa_modem.rs:527-557`) applies, in order:

1. **Section RMS leveling.** The frame is split into 3 sections —
   `[0, 2*sym)` (preamble), `[2*sym, 3*sym)` (probe), `[3*sym, end)`
   (header+payload body), where `sym = fft_size + cp_samples` — and each
   section's RMS is independently scaled to match the body section's RMS
   (`crates/coppa-codec/src/ofdm/coppa_modem.rs:545-550`). This is
   **mandatory for every profile** (HF and VHF), not HF-specific: without
   it, the preamble's unit-RMS normalization (§3.1) leaves it ~30–34 dB
   hotter than the body's naturally quiet sparse-bin IFFT output, starving
   the header/payload of virtually all injected noise budget under a
   whole-frame-mean-power SNR convention (documented failure mode,
   `crates/coppa-codec/src/ofdm/coppa_modem.rs:527-544`).
2. **Raised-cosine inter-symbol taper (`rc_overlap`).** Every symbol
   boundary is cross-faded over `RC_OVERLAP = 24` samples (0.5 ms @ 48 kHz)
   (`crates/coppa-codec/src/ofdm/coppa_modem.rs:109-110,1479-1493`), a
   raised-cosine taper worth ~15–20 dB of out-of-band sidelobe suppression
   per its doc comment.
3. **PAPR clip** at the speed level's `papr_target_db` (§6 table)
   (`papr_clip`, `crates/coppa-codec/src/ofdm/mod.rs:421-450`): samples
   whose magnitude exceeds `RMS * 10^(target_db/20)` are hard-clipped to
   that threshold, sign preserved.
4. **TX bandpass filter — HF profiles only** (`phy_mode == 0`): a
   **601-tap, linear-phase, Blackman-windowed-sinc bandpass**, passband
   **250–2850 Hz** at the profile's sample rate
   (`crates/coppa-codec/src/ofdm/coppa_modem.rs:299-306`;
   `design_bandpass`, `crates/coppa-dsp/src/fir.rs:1-24`, window
   `0.42 - 0.5*cos(x) + 0.08*cos(2x)`). **Not applied** to VHF profiles
   (`phy_mode == 1`) — their carrier band (up to ~5.9 kHz) and shorter CP
   (60 samples) are incompatible with this filter's passband/group delay
   (`crates/coppa-codec/src/ofdm/coppa_modem.rs:183-203`). Note this 601-tap
   filter's 250–2850 Hz passband is a *TX/RX conditioning* parameter,
   distinct from (and deliberately wider than) the *carrier placement*
   band computed in §1.2 (e.g. 350–2700 Hz for `hf_standard`) — the two
   numbers serve different purposes and are not expected to match.
5. **Peak normalize** to `TX_PEAK = 0.5` (fraction of full scale)
   (`crates/coppa-codec/src/ofdm/coppa_modem.rs:117,1495-1504`): the whole
   frame's samples are scaled so the single largest-magnitude sample equals
   exactly `0.5`.

A conforming transmitter **MUST** apply steps 1, 2, 3, and 5 for every
profile, and step 4 for HF profiles only. A conforming receiver is not
required to replicate any specific TX-side conditioning internally, only to
correctly demodulate its result — an RX-side bandpass filter matching step
4's passband is used by the reference receiver
(`crates/coppa-protocol/src/modem/transceiver.rs:528-535`) but this is a
receiver strategy choice (**SHOULD**, not **MUST**).

---

## 11. Byte/bit orders — cross-reference

| Structure | Order | Source |
|---|---|---|
| `CoppaHeader` 6-byte packing | MSB-first per field and per byte | `crates/coppa-codec/src/ofdm/frame.rs:280-292` |
| Header CRC-16 (`CRC_16_IBM_SDLC`) | appended MSB-first as 16 info bits | `crates/coppa-codec/src/ofdm/header_fec.rs:79,88-90` |
| Header FEC info-bit packing (bytes → bits) | MSB-first (`(byte >> shift) & 1`, `shift` from 7 down to 0) | `crates/coppa-codec/src/ofdm/header_fec.rs:83-86` |
| Golay(24,12) codeword packing | MSB-first (bit 23 down to 0) | `crates/coppa-codec/src/ofdm/header_fec.rs:101-104` |
| Payload CRC-32 (`CRC_32_ISO_HDLC`) trailer | appended **big-endian** (`to_be_bytes()`) | `crates/coppa-protocol/src/modem/transceiver.rs:689-692` |
| Payload info-bit packing (bytes → bits, pre-LDPC) | MSB-first | `crates/coppa-protocol/src/modem/transceiver.rs:695-699` |
| `Callsign::encode` (8 chars × 6 bits → 6 bytes) | packed MSB-first, 6-bit codes concatenated big-endian across byte boundaries | `crates/coppa-protocol/src/mac.rs:100-123` (explicit bit-layout diagram at `:108-114`) |
| `MacPdu::to_bytes` | byte 0 = `[version:4][frame_type:4]` (MSB-first), then dest (6B), src (6B), ssid (1B, low nibble), payload | `crates/coppa-protocol/src/mac.rs:210-229` |
| PRBS scrambler LFSR | `x^15 + x^14 + 1`, seed `0x4A80`, self-inverse XOR | `crates/coppa-protocol/src/fec/scrambler.rs:6-16` |
| Preamble PN LFSR (`coppa_pn_sequence`) | 7-bit, `x^7 + x + 1`, seed `(version+1) & 0x7F` | `crates/coppa-codec/src/ofdm/sync.rs:21-51` |

The MAC layer (`crates/coppa-protocol/src/mac.rs`) is **not** part of the
PHY/FEC/header wire format this spec otherwise covers (§1–§10); it is
included here only because the brief's "byte/bit orders everywhere"
requirement extends to it — `MacPdu` is Coppa's link-layer PDU, carried as
the OFDM payload's application bytes (§5.5's "payload"), not a PHY-layer
structure.

---

## 12. Discrepancy note: stale `CodeRate` label for speed level 10

`speed_level_components(10)` returns `CodeRate::Rate7_8`
(`crates/coppa-protocol/src/modem/speed_levels.rs:24`, confirmed by test
`crates/coppa-protocol/src/modem/speed_levels.rs:249`), a legacy-codec
label (§5.1) that no longer matches level 10's *real* rate. Per §5.2,
level 10's actual `k_used = 1620` is rate **5/6**, not 7/8 — the 7/8→5/6
change was a deliberate wire-format break (§0.1, §5.2). Tracing the call
site (`CoppaTransceiver::new`,
`crates/coppa-protocol/src/modem/transceiver.rs:547-560`) confirms this
`CodeRate` return value is destructured as `_code_rate` and **discarded** —
it has no effect on the actual rate-matching pipeline (which uses only
`k_used_for_level`, §5.2). This is dead/stale metadata, not a wire-format
bug: it affects no transmitted bit. An implementer should use §5.2's
`k_used` table as the source of truth for level 10's rate, not this enum
label.

---

## 13. Conformance checklist

A conforming **transmitter** MUST:

- [ ] Use one of the `CoppaProfile`s in §1.1 (at minimum `hf_standard`),
      with `carrier_offset = 6` and the exact FFT/CP/carrier-count values
      listed.
- [ ] Emit a 2-symbol Newman-phase comb preamble (§3.1) on even in-band FFT
      bins, phase-rotated by the protocol version, unit-RMS normalized,
      repeated identically twice.
- [ ] Emit one full-comb version-keyed PN probe symbol immediately after
      the preamble (§3.2).
- [ ] Emit the 48-bit header (§4.2) protected exactly as specified in §4.3
      (CRC-16, zero pad, Golay(24,12) with the `A_ROWS` generator, stride-5
      interleave, BPSK).
- [ ] Encode payload via the NR BG2 mother code (§5.1), shortened to the
      speed level's `k_used` (§5.2), scrambled with the specified PRBS
      (§5.4), CRC-32-protected per codeword (§5.5), rate-matched to 1944
      coded bits per codeword at the frame's declared `rv` (§5.2–§5.3).
- [ ] Apply the §7.1 block interleaver per codeword, and (for
      `codewords > 1 && speed_level >= 5`) the §7.2 cross-codeword
      interleaver, before constellation mapping (§6).
- [ ] For multi-codeword frames, split payload bytes per §9.1's exact
      algorithm and never exceed `MAX_CODEWORDS = 8`.
- [ ] Apply all of §10's TX conditioning steps (section leveling, RC
      taper, PAPR clip at the level's target, HF-only 601-tap 250–2850 Hz
      bandpass, peak-normalize to `TX_PEAK = 0.5`).
- [ ] Use the pilot pattern of §3.3 (even/odd alternating positions, `+1`
      pilot values) on every header and payload OFDM symbol.

A conforming **receiver** MUST:

- [ ] Be configured (out-of-band) with the same `CoppaProfile` and protocol
      `version` the transmitter used (§1.3) — this codec does not
      negotiate or auto-detect the profile in-band.
- [ ] Correctly demodulate a real-valued Hermitian-symmetric OFDM signal at
      the profile's exact FFT size, cyclic prefix, and active-carrier
      layout (§1–§2).
- [ ] Tolerate at least a ±50 Hz carrier frequency offset (the coarse
      two-stage Moose estimate's unambiguous range at `fft_size=960`,
      `sample_rate=48000`: `±sample_rate/fft_size`,
      `crates/coppa-codec/src/ofdm/sync.rs:202-206`) — beyond this the
      ambiguity-resolution itself wraps (documented limitation, see
      CLAUDE.md).
- [ ] Correctly decode the protected header per §4.3 (at minimum the hard
      Golay(24,12)+CRC-16 path; soft/LLR decoding is recommended for real
      HF-fading robustness but not required for wire conformance).
- [ ] Correctly decode NR BG2 payload codewords per §5.1–§5.4 (any correct
      LDPC decoder for this base graph/lifting suffices; the specific
      normalized-min-sum decoder in §5.6 is not required).
- [ ] Verify each codeword's CRC-32 trailer (§5.5) and reject on mismatch.
- [ ] Correctly invert both interleaving stages (§7.1–§7.2) using the exact
      permutations specified, and correctly recover per-codeword payload
      byte ranges via §9.1's split algorithm using the header's
      `payload_len`/`codewords` fields.

A conforming implementation MAY:

- [ ] Omit support for profiles beyond `hf_standard` (§1.1's other rows are
      SHOULD, not MUST).
- [ ] Use a different internal channel-estimation strategy than this
      reference implementation's delay-domain/Kalman estimators (§3.2's
      informative note) — only the demodulated bits must match.
- [ ] Choose not to implement redundancy-version cycling / IR-HARQ transmit
      logic (§5.3) as long as it correctly decodes `rv=0` single-shot
      frames and (if it supports retransmission at all) uses the specified
      RV values on the wire.
- [ ] Choose not to support multi-codeword frames (§9) beyond decoding
      `codewords=1` correctly, though it MUST still parse the `codewords`
      header field correctly (§4.2) to know how many codewords follow.

---

## 14. Executable conformance vectors

`testdata/golden/manifest.toml` (20 entries: the cross product of levels
`{1, 2, 5, 6, 9}` × channel conditions `{clean, awgn12, poor25, ssbcfo}`,
enforced by `golden_manifest_covers_the_full_grid`,
`crates/coppa-protocol/tests/golden_vectors.rs:205-232`) is a set of
**frozen, committed reference vectors** for exactly this spec's wire
format: each entry names a 48 kHz/16-bit-PCM WAV file
(`testdata/golden/<id>.wav`) containing one complete Coppa frame (encoded
per this spec, then passed through a documented channel condition —
`clean`, 12 dB AWGN, a 25 dB "poor" Watterson-fading preset, or a 20 dB SSB
+15 Hz-CFO condition) plus the exact application payload
(`payload_hex`) that frame **MUST** decode back to.

`crates/coppa-protocol/tests/golden_vectors.rs`'s
`golden_vectors_decode_to_expected_payloads` test
(run via `cargo test -p coppa-protocol --test golden_vectors`, or as part
of `cargo test --workspace`) is the executable check: for every manifest
entry it constructs a `CoppaTransceiver` for that level's profile, calls
`receive()` on the WAV samples, and asserts the recovered `speed_level` and
payload bytes exactly match the manifest (`crates/coppa-protocol/tests/golden_vectors.rs:144-190`).
One entry (`L9_poor25`) has `expected_decode_ok = false` — a **documented,
verified known-limitation failure** (level 9's steep, seed-dependent AWGN
threshold under fading, per CLAUDE.md's Known Limitations), not a generator
bug; the test asserts this entry *fails* to decode, as a tripwire against
that failure silently disappearing or a *different* vector silently
starting to fail.

An independent implementer **SHOULD** treat this manifest + WAV set as the
primary interoperability conformance suite: correctly demodulating every
`expected_decode_ok = true` WAV to its exact `payload_hex`, and correctly
*failing* to decode `L9_poor25`, is strong evidence of wire-format
compatibility with this reference implementation. Vectors are regenerated
(not hand-authored) via `cargo run -p coppa-bench --release --example
golden_vectors_gen` (`crates/coppa-protocol/tests/golden_vectors.rs:3-4,129`).
