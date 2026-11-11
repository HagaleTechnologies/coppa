# Mercury 1.9.23 air protocol: clean-room functional description and interoperability assessment

## 1. Status, scope and provenance

Proposed. Not normative for coppa. Awaiting the owner's decision on section 7 and section 8.

This functional description is an input for an independent implementer. It cannot yet produce a frame accepted by Mercury. Section 5 records the gaps rather than supplying guessed constants. Implementation stays in Backlog until the owner accepts the recommendation and the conditions in section 8.7 hold.

The research and implementation agents read public prose documents only, following decisions 3, 4 and 7 of `docs/DECISIONS/2026-11-09-clean-room-and-licensing-policy.md`. No Mercury or codec2 source, formal models, Doxygen HTML or issues were read. No Mercury binary was built or run. The ticket's permission for a human specification author to read source was not exercised. The codec2 spreadsheet was read as document cells and labels. Its unpinned `main` revision fetched on 2026-11-10 is a provenance limitation.

This file is coppa's own description, licensed MIT OR Apache-2.0 like the repository. It contains no copied code, source comments or transcribed pseudo-code, and no Mercury or codec2 text beyond short phrases and user-visible names. Numeric interoperability facts are stated as facts. Field descriptions use independent wording. No cross-implementation or over-the-air result is claimed. Digest-only claims from the research were rechecked against Mercury's pinned public documents during assembly on 2026-11-11.

- **[D]** stated in a cited public document.
- **[V]** derived by arithmetic from documented numbers. The arithmetic is shown.
- **[I]** inferred, not verified. Treat as a hypothesis.
- **[C]** the cited documents disagree. See the conflict register (section 6).

Mercury is a GPL-3.0 project. The presentation DECK declares CC-BY-SA-4.0. This document paraphrases its facts rather than reproducing its text. All Mercury paths are under `https://github.com/Rhizomatica/mercury/blob/v1.9.23/`.

| Key | Document |
|---|---|
| CARQ | [docs/CAROUSEL-ARQ.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/CAROUSEL-ARQ.md) (overview and wire formats) |
| CSPEC | [docs/CAROUSEL-SPEC.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/CAROUSEL-SPEC.md) (rule-by-rule specification; sections 1 to 11 derived at an older commit, sections 12 to 21 record later changes) |
| CTURN | [docs/CAROUSEL-TURNS.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/CAROUSEL-TURNS.md) (turn-taking rules and model) |
| CREP | [docs/CAROUSEL-REPORT.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/CAROUSEL-REPORT.md) |
| ARQ | [docs/ARQ.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/ARQ.md) (mostly 1.9.x history; connection states and CQ still current) |
| MODES | [docs/MODES.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/MODES.md) |
| MFSKP | [docs/MFSK-PORT.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/MFSK-PORT.md) |
| ACQW, ACQT, OFFS | [docs/ACQUISITION-WINDOWS.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/ACQUISITION-WINDOWS.md), [docs/ACQ-TUNING-FINDINGS.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/ACQ-TUNING-FINDINGS.md), [docs/OFFSET-ACQUISITION-FINDINGS.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/OFFSET-ACQUISITION-FINDINGS.md) |
| HARQ, CLIP | [docs/HARQ-FINDINGS.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/HARQ-FINDINGS.md), [docs/CLIPPED-QAM16C2-FINDINGS.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/CLIPPED-QAM16C2-FINDINGS.md) |
| BWF, BFILE | [docs/BROADCAST-WIRE-FORMAT.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/BROADCAST-WIRE-FORMAT.md), [docs/BROADCAST-FILE.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/BROADCAST-FILE.md) |
| TNC | [docs/TNC.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/TNC.md) |
| REL-1.9.18 to REL-1.9.23, UP-1.9.18 | [docs/RELEASE-1.9.18.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/RELEASE-1.9.18.md) to [docs/RELEASE-1.9.23.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/RELEASE-1.9.23.md), [docs/UPGRADING-TO-1.9.18.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/UPGRADING-TO-1.9.18.md) |
| README, DECK | [README.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/README.md), [docs/presentations/2026-10-mercury/mercury.tex](https://github.com/Rhizomatica/mercury/blob/v1.9.23/docs/presentations/2026-10-mercury/mercury.tex) |
| S-CONN, S-TEAR, S-TURN, S-RUNG, S-CAR | [specs/connect/README.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/specs/connect/README.md), [specs/teardown/README.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/specs/teardown/README.md), [specs/turns/README.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/specs/turns/README.md), [specs/rungs/README.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/specs/rungs/README.md), [specs/carousel/README.md](https://github.com/Rhizomatica/mercury/blob/v1.9.23/specs/carousel/README.md) (prose only) |
| C2DATA, C2OFDM, C2ODS | codec2 [README_data.md](https://github.com/drowe67/codec2/blob/main/README_data.md), [README_ofdm.md](https://github.com/drowe67/codec2/blob/main/README_ofdm.md), [doc/modem_codec_frame_design.ods](https://github.com/drowe67/codec2/blob/main/doc/modem_codec_frame_design.ods) (LGPL-2.1 project, `main` fetched 2026-11-10) |

The spec covers what a station must transmit and accept to hold an ARQ session with Mercury 1.9.23 over HF audio. It does not cover Mercury's host interfaces (they are VARA-style TCP and already have counterparts in `coppa-host`), its GUI, its encryption (not shipped in 1.9.20 to 1.9.23), or its receiver internals, except where an internal decision changes what appears on the air.

## 2. Pinned release and compatibility window

- **Pinned release:** Mercury v1.9.23, published 2026-11-09, tag commit `09f258f84a0b69cb4d8556530a25822617ea17dd`, default branch `mercuryv2`, GPL-3.0 (GitHub release metadata, REL-1.9.23).
- **On-air compatibility window:** 1.9.20, 1.9.21, 1.9.22 and 1.9.23 connect to each other in both directions. 1.9.19 and older do not connect to any of them (REL-1.9.23, REL-1.9.22, REL-1.9.21, REL-1.9.20).
- **Breaks:** 1.9.18 (new data plane, versioned connect frames), 1.9.19 (MFSK tones moved to centre on 1500 Hz, version 2) and 1.9.20 (new connect packet type, "2.0" architecture) each refuse older stations (REL-1.9.18, REL-1.9.19, REL-1.9.20).
- **Additions without a break:** 1.9.21 used one bit of the connect extension for "sender transmits only MFSK" (REL-1.9.21). 1.9.23 added a station-message frame, packet type 7 (REL-1.9.23). 1.9.22 changed nothing on the wire (REL-1.9.22).
- A second implementer should target the 1.9.20 to 1.9.23 wire and treat it as one generation, which Mercury's README calls the "2.0" architecture (README, "On-air compatibility").

| Release | Date | Wire change | Connects to |
|---|---|---|---|
| 1.9.17 and older | before 2026-10-12 | n/a | none of the below |
| 1.9.18 | 2026-10-12 | break (carousel data plane, versioned CALL) | not 1.9.17 or older |
| 1.9.19 | 2026-10-13 | break (MFSK moved to 1500 Hz centre, version 2) | not 1.9.18 or older |
| 1.9.20 | 2026-10-22 | break (CALL2 packet type, "2.0") | not 1.9.19 or older |
| 1.9.21 | 2026-10-24 | additive (MFSK-only bit in minor revision 1) | 1.9.20 |
| 1.9.22 | 2026-11-03 | none | 1.9.21, 1.9.20 |
| 1.9.23 | 2026-11-09 | additive (station frame, type 7) | 1.9.22, 1.9.21, 1.9.20 |

Sources: REL-1.9.18 to REL-1.9.23, UP-1.9.18, CARQ, README. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `README.md` (v1.9.23); Mercury `docs/UPGRADING-TO-1.9.18.md` (v1.9.23); Mercury `docs/RELEASE-1.9.18.md` (v1.9.23); Mercury `docs/RELEASE-1.9.23.md` (v1.9.23).

## 3. Physical layer

### 3.1 Common audio and timing

- **Sample rate:** 8 kHz for the OFDM data modes and for MFSK. [D] MODES and C2OFDM examples use 8000 Hz. The MFSK study geometry is 8000 Hz (MFSKP). Mercury's test audio backend uses 8 kHz raw samples (README).
- **Centre frequency:** every mode is centred on 1500 Hz audio, so one radio passband setting suits all. MFSK tones span 1015.6 to 1984.4 Hz. [D] MODES. This replaced a 2000 Hz carrier in 1.9.19 and is the reason 1.9.19 refused 1.9.18 (REL-1.9.19).
- **Signal:** real passband, 16-bit samples (MFSKP).
- **Burst structure:** each transmitted frame is one burst: preamble, one frame, postamble. A "round" is one keydown holding up to N bursts, each with its own preamble and postamble. [D] CARQ "How it works".
- **Burst spacing inside a keydown:** 100 ms between bursts, 200 ms for QAM16C2. With no gap the receiver misses bursts after the first. [D] CARQ "Wire formats".
- **Keydown length:** at most 30 s. A station may hold its own lower with `arq:max_keydown_s` (0, or 5 to 30 s). The 27 s BW500 floor frame is never split. [D] CSPEC section 20, TNC ("CANCELPENDING"), REL-1.9.22.
- **Carrier accuracy:** OFDM modes acquire within about +/-55 Hz of nominal. MFSK acquires within about +/-100 Hz. 10 ppm at 14 MHz is 140 Hz, outside both, so a transmitter's CAT frequency must be accurate to a few tens of hertz. [D] ACQW, README docs index.

Sources: CARQ, CSPEC sections 1 and 20, MODES, MFSKP, ACQW, REL-1.9.19, REL-1.9.22. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/MODES.md` (v1.9.23); Mercury `docs/MFSK-PORT.md` (v1.9.23); Mercury `docs/ACQUISITION-WINDOWS.md` (v1.9.23); Mercury `docs/RELEASE-1.9.19.md` (v1.9.23); Mercury `docs/RELEASE-1.9.22.md` (v1.9.23).

### 3.2 OFDM data modes

Mercury's OFDM modes are codec2's raw-data modes plus Mercury additions. codec2's public design spreadsheet and READMEs give the structural parameters. The pilot, unique-word and preamble sequences, the bit mapping and the LDPC matrices are published only as code (G-08, A-1 to A-3, A-9).

**Common structure [D, with [V] marks].**

- Carrier spacing is 62.5 Hz. The useful symbol is 16 ms plus cyclic prefix 6 ms (4 ms for QAM16C2), so the transmitted symbol interval is 22 ms (20 ms), or about 45.45 (50) symbols/s [V]. At 8 kHz the useful part is 128 samples, so carrier spacing is 62.5 Hz. [V] 8000 x 0.016 = 128. DATAC14 differs: 55.56 Hz carrier spacing, 18 ms plus 5 ms, about 43.48 transmitted symbols/s [V]. [D] C2ODS.
- Bandwidth is the carrier count times 62.5 Hz. [V]
- A modem frame is 5 OFDM symbols, 110 ms (100 ms for QAM16C2). One symbol of the five is a pilot symbol and four carry data, so data symbols per modem frame equal 4 times the carrier count. [V] C2ODS lists 36 data symbols per frame for 9 carriers and 16 for 4.
- A data frame is `payload + 2 CRC bytes`, LDPC encoded, then a unique word (UW) of tens of symbols is placed outside the codeword. [D] C2DATA, C2ODS.
- Check on payload sizes: payload bytes plus 2 equals data bits divided by 8 for every mode in the table below. [V] For example DATAC3 has 126 + 2 = 128 bytes = 1024 bits, and its code is (2048, 1024).
- A burst has a preamble and a postamble. A decoder may sync on either, which gives time diversity in fading. [D] C2DATA.
- Mercury ARQ sends one data frame per preamble. [D] HARQ.
- Total airtime of a frame with preamble and postamble is about 0.6 to 0.66 s more than the frame itself. [V] Frame durations in MODES plus 0.66 s match the airtimes in CSPEC section 1.3 for DATAC15 (3.74 + 0.66 = 4.40 s), DATAC16 (3.08 + 0.66 = 3.74 s), DATAC4, DATAC3, DATAC1 and DATAC17.

| Mode | Index | Origin | Carriers | RF bandwidth | Modulation | UW | FEC (n, k) | Payload | Frame (s) | Airtime incl. sync (s) |
|---|---|---|---|---|---|---|---|---|---|---|
| DATAC0 | 2 | codec2 | 9 | 562.5 Hz | QPSK | 16 symbols | (256, 128) | 14 B | 0.44 | not stated |
| DATAC1 | 0 | codec2 | 27 | 1687.5 Hz | QPSK | 8 symbols | (8192, 4096) | 510 B | 4.18 | 4.81 |
| DATAC3 | 1 | codec2 | 9 | 562.5 Hz | QPSK | 20 symbols | (2048, 1024) | 126 B | 3.19 | 3.82 |
| DATAC4 | 3 | codec2 | 4 | 250 Hz | QPSK | 16 symbols | (1472, 448) | 54 B | 5.17 | 5.80 |
| DATAC13 | 4 | codec2 | 3 | 187.5 Hz | QPSK | 24 symbols | (384, 128) | 14 B | 1.98 | not stated |
| DATAC14 | 5 | codec2 | 4 | 222.2 Hz | QPSK | 16 symbols | (112, 56), shortened to 40 data bits | 3 B | 0.46 | 0.69 |
| DATAC15 | 7 | Mercury | 3 [I] | 187 Hz | QPSK [I] | 48 bits [D] ACQT | (768, 256) rate 1/3 | 30 B | 3.74 | 4.40 |
| DATAC16 | 8 | Mercury | 3 [I] | 187 Hz | QPSK [I] | 32 bits [D] ACQT | (640, 128) after shortening | 14 B | 3.08 | 3.74 |
| DATAC17 | 9 | Mercury | 33 [V] | 2062 Hz | QPSK | 168 bits [V] | (15936, 9456) after shortening | 1180 B | 6.71 | 7.40 |
| QAM16C2 | 10 | ported from the Rhizomatica codec2 fork | 33 | 2062.5 Hz | 16-QAM | 42 symbols | (16200, 9720), rate 0.6, DVB-S2 code [D] C2ODS | 1213 B | 3.10 [C] | 3.70 |

Notes on the table:

- DATAC0 to DATAC4, DATAC13, DATAC14 and QAM16C2 values come from C2ODS and C2DATA. Mercury's `MODES.md` repeats the payload sizes and FEC pairs. Index numbers are Mercury's (MODES).
- DATAC14: C2DATA lists (112, 56) and 3 B. The spreadsheet's candidate shows 96 coded bits and 40 data bits, which is the same code after shortening. [V] 3 + 2 bytes = 40 bits.
- DATAC15 and DATAC16 are described as "3-carrier narrowband" with a rate-1/3 LDPC code listed for DATAC15 and DATAC16 (MODES). The 3-carrier geometry equals DATAC13's. The structure is consistent with these durations [V]: DATAC15 has (768 + 48 UW bits) / 24 data bits per modem frame = 34 frames x 0.11 s = 3.74 s. DATAC16 has (640 + 32) / 24 = 28 frames x 0.11 s = 3.08 s. The carrier count and symbol timing are not stated for these two. [I]
- DATAC17 is "QPSK on QAM16C2's codeword", shortened so the frame stays distinct in size from QAM16C2's (MODES). If it uses 33 carriers with 6 ms CP, 264 data bits per modem frame, then 61 frames x 0.11 s = 6.71 s matches. [V] The UW length 168 bits is the remainder 61 x 264 - 15936. [V, low confidence]
- QAM16C2's duration differs by document: 3.10 s in C2ODS, 3.2 s in MODES, 3.30 s in CLIP. [C]
- DATAC1, DATAC3, DATAC4, DATAC15 and DATAC17 are ARQ rungs. DATAC0, DATAC13 and DATAC14 are broadcast and test modes only. FSK_LDPC (index 6) is experimental and unused in ARQ (MODES).

**Transmit conditioning that does not change the wire format.** codec2 clips by default to raise average power. Mercury documents an optional clipper for QAM16C2 (drive 2.0 and 1.0 cuts peak-to-average from 11.4 to 6.7 dB) that needs no change on the receiver (CLIP, C2DATA). A coppa transmitter may use its own conditioning.

**Acquisition (receiver behaviour that sets what a transmitter must provide).**

- OFDM modes: Mercury forces burst acquisition for every mode. A coarse preamble correlation steps 5 Hz across +/-50 Hz, then a fine search covers +/-3 Hz in 1 Hz steps. The capture window is +/-55 Hz for every mode. [D] OFFS, ACQW.
- A timing metric is compared with a per-mode threshold before the UW is checked. Thresholds: 0.08 DATAC0, 0.10 for DATAC1, DATAC3 and DATAC17, 0.30 QAM16C2, 0.45 DATAC13 and DATAC14, 0.50 DATAC4, and 0.40 for DATAC15 and DATAC16 (lowered from 0.45 in 1.9.23). [D] ACQW, ACQT, REL-1.9.23.
- UW error tolerance: DATAC16 accepts up to 12 wrong bits of 32. DATAC15 accepts up to 18 of 48. [D] ACQT.
- Acquisition dominates the weak-signal floor for the narrow modes. Frames that sync decode 92 to 99 percent of the time down to -11 dB SNR3k. [D] MODES.

Sources: C2ODS tabs "ofdm data c0", "c1_2", "c3_2", "c4", "c13", "c14", "qam16c2"; C2DATA; C2OFDM; MODES; CSPEC section 1.3; ACQW; ACQT; OFFS; CLIP; HARQ. Paths: Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/MODES.md` (v1.9.23); Mercury `docs/ACQUISITION-WINDOWS.md` (v1.9.23); Mercury `docs/ACQ-TUNING-FINDINGS.md` (v1.9.23); Mercury `docs/OFFSET-ACQUISITION-FINDINGS.md` (v1.9.23); Mercury `docs/HARQ-FINDINGS.md` (v1.9.23); Mercury `docs/CLIPPED-QAM16C2-FINDINGS.md` (v1.9.23); codec2 `README_data.md` (main fetched 2026-11-10); codec2 `README_ofdm.md` (main fetched 2026-11-10); codec2 `doc/modem_codec_frame_design.ods` (main fetched 2026-11-10).

### 3.3 MFSK floor modes and tone patterns

MFSK is a non-coherent 32-tone FSK modem ported from Mercury 1.x. It is rung 0 of the ladder and carries control frames and CALLs when the OFDM modes are out of reach. It is Mercury's own design, not codec2's. [D] MODES, MFSKP.

| Parameter | MFSK (1 kHz) | MFSK500 |
|---|---|---|
| Mode index | 11 | 12 |
| Tones | 32 | 32 |
| Tone spacing | 31.25 Hz [D] MODES | about 15.6 Hz [D] CARQ |
| Span | 1015.6 to 1984.4 Hz [D] MODES | about 500 Hz, centred on 1500 Hz [D] CARQ |
| Symbol duration | 40 ms (25 symbols/s) [D] MFSKP | 80 ms [D] CARQ |
| Bits per symbol | 5 [D] MFSKP | 5 [V] |
| Frame payload | 98 B | 98 B |
| FEC | LDPC rate 8/16, N = 1600, so K = 800 bits = 100 B = 98 B + 2 CRC [D] MODES [V] | same |
| Airtime | about 13.5 s (13.1 to 13.5 s in different documents [C]) | 26.2 s (26 s, 27 s and 27.6 s in different documents [C]) |
| Data symbols | 1600 / 5 = 320, 12.8 s [V] | 320 x 80 ms = 25.6 s [V] |
| Threshold at 90 percent delivery | -11.1 dB SNR3k on AWGN | -14.1 dB |

- The mapping uses Gray coding plus a tone-hopping scheme. The sequences are not published (G-07, A-6).
- The interleaver scatters the 1600-bit codeword across the whole burst. It is a Fisher-Yates shuffle with a fixed xorshift seed, integer-only so both ends derive the same table. [D] MFSKP. The seed and generator are not published (G-07, A-5).
- The LDPC code is a systematic IRA code decoded with min-sum. Matrix not published (A-4). [D] MFSKP
- A preamble tone sequence and a distinct postamble tone sequence let the receiver sync from either end. The postamble gained about 2 dB under fading. Lengths and tones are not published (G-07, A-7). [D] MFSKP
- Symbols are cross-faded with a raised cosine (out-of-band emission fell from -27 to -71 dB). [D] CREP
- Sync accept gate: a normalised correlation of 0.08, about 1.6 times the measured noise-only maximum. A CRC failure releases the sync anchor. [D] MFSKP
- Frequency search: +/-94 Hz in half-bin steps (about 15.6 Hz) for the 1 kHz mode, with the measured capture window +/-100 Hz. [D] MFSKP, ACQW
- A frame carries 6 bytes of headers and 4 pieces of 23 bytes (92 B), or 3 pieces of 24 bytes when the session does not start on the floor. [D] CARQ [V] 6 + 92 = 98.
- A listening station decodes both floors (its own and the other bandwidth's). A BW500 floor frame is sent alone in a keydown. [D] CARQ, CSPEC section 17, REL-1.9.22

**Tone patterns (signalling without a coded frame).** 16 symbols long, 0.64 s at the 1 kHz floor. Symbol i uses the tone `(list[i mod 8] + 13 x i) mod 32`, where `list` is an 8-tone list. A receiver counts symbols whose expected tone is the spectral peak, searching +/-2 bins. [D] CARQ "Wire formats".

| Pattern | Tone list | Detected at |
|---|---|---|
| ACK | the session's list, or a global list outside a session | 8 of 16 symbols |
| BREAK | the session's list, or a global list outside a session | 8 of 16 |
| NAV class k (k = 0 to 11) | one list per class | 9 of 16 |

- Session lists are derived deterministically from the session seed so only the two stations of a session read each other's patterns. The derivation is not published (G-09, A-8). [D] CARQ, CSPEC section 12
- Any two lists agree in at most 3 symbols (NAV classes with each other in at most 4), at any time shift and up to 4 bins of frequency shift. [D] CARQ
- NAV classes announce how long a keydown lasts: 12 classes from 2 s to 32 s, each 1.287 times the last. [D] CSPEC section 12
- Detection reaches about -14 dB SNR3k on AWGN, where DATAC16 frames stop near -10 dB. [D] CARQ
- Docs disagree on whether NAV headers are sent during a session. CARQ says every keydown below 0 dB opens with one. CTURN says they are not sent in a session and may stay for third parties. [C]
- The pattern length at the BW500 floor is not stated. [G-09]

Sources: MODES, MFSKP, CARQ ("The BW500 floor", "Carrier sense and the NAV header", "Wire formats"), CSPEC sections 12 and 17, CREP, ACQW, REL-1.9.22. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/CAROUSEL-REPORT.md` (v1.9.23); Mercury `docs/MODES.md` (v1.9.23); Mercury `docs/MFSK-PORT.md` (v1.9.23); Mercury `docs/ACQUISITION-WINDOWS.md` (v1.9.23); Mercury `docs/RELEASE-1.9.22.md` (v1.9.23).

### 3.4 Where Mercury relies on FreeDV and codec2 (LGPL-2.1)

Mercury vendors a subset of codec2 under `modem/freedv/` with its own licence file (README "LICENSE"). The combined binary is GPL-3.0. Which parts a clean-room implementer can describe from codec2's public documentation, and which exist only as LGPL code:

| Element | Public documentation | Only in LGPL code | Interop-critical |
|---|---|---|---|
| OFDM structure (symbol rate, carriers, CP, pilot spacing, UW length, frame counts) | C2ODS per-mode tabs, C2DATA | no | yes |
| Pilot symbol values and carrier index map | no | yes | yes |
| UW bit patterns | lengths only | yes | yes |
| Preamble and postamble waveform | purpose and use only (C2DATA) | yes | yes |
| Bit-to-symbol mapping, scrambling or interleaving of the codeword | no | yes | yes |
| LDPC matrices: stock codes | (n, k) only | yes | yes |
| LDPC matrix: QAM16C2 and DATAC17 | the spreadsheet says "DVBS2 code" at rate 0.6, (16200, 9720) | not if it is the DVB-S2 table | yes |
| 16-bit CRC | "16-bit checksum" | yes (polynomial) | yes |
| TX clipper and TX filter | behaviour (C2DATA) | yes | no |
| SNR estimator | behaviour and a note that clipping biases it | yes | no (local policy) |
| Octave reference modem | named in C2OFDM | LGPL source | n/a |

[D] The (16200, 9720) parameters match the DVB-S2 short-frame rate-3/5 LDPC code in [ETSI EN 302 307-1 V1.4.1 (2014-11)](https://www.etsi.org/deliver/etsi_en/302300_302399/30230701/01.04.01_60/en_30230701v010401p.pdf), clause 5.3, Table 5b and clause 5.3.2. This confirms the parameter match, not Mercury's exact matrix or shortening order. C2ODS also labels it DVB-S2. [I] Using the standard's construction for Mercury remains unverified until vectors or an authorised description confirm identity. A-3 still withholds all matrix data. No BCH outer code is inferred from the LDPC parameter match.

Mercury's own acquisition search and the MFSK modem are Mercury code (GPL-3.0), not codec2. They are receiver internals except for the MFSK waveform definition.

Sources: README ("Physical Layer", "LICENSE"), MODES, C2DATA, C2OFDM, C2ODS, MFSKP. Paths: Mercury `docs/MODES.md` (v1.9.23); Mercury `docs/MFSK-PORT.md` (v1.9.23); Mercury `README.md` (v1.9.23); codec2 `README_data.md` (main fetched 2026-11-10); codec2 `README_ofdm.md` (main fetched 2026-11-10); codec2 `doc/modem_codec_frame_design.ods` (main fetched 2026-11-10).

## 4. Link and session protocol

### 4.1 Layers

Mercury's stack, from the bottom:

1. **Waveforms.** Ten OFDM data modes from codec2's FreeDV raw-data family and Mercury's own additions, plus two 32-tone MFSK modes. All run at 8 kHz audio, centred on 1500 Hz.
2. **Frame check.** Every frame carries a 16-bit CRC. In a session the CRC is XORed with a session seed, so frames from other sessions fail like any corrupt frame.
3. **Frames.** Plain framed packets use a 3-bit type; seeded carousel traffic has separate DATA and control layouts.
4. **Session.** CALL and ACCEPT open it, a receiver-driven erasure-coded "carousel" moves data, and DISCONNECT ends it.
5. **Turn discipline.** The caller is the only station that keys on its own timers. The callee keys only in slots that follow a caller keydown.
6. **Rate ladder.** Seven rungs, chosen by the receiver of each direction.

Sources: CARQ (intro, "How it works"), CTURN section 2, README ("Physical Layer"). Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-TURNS.md` (v1.9.23); Mercury `README.md` (v1.9.23).

### 4.2 Frames and checks

**Framer byte outside the carousel.** Byte 0 of a plain framed packet holds a 3-bit packet type in bits 7 to 5 and a 5-bit type-specific extension in bits 4 to 0. [D] ARQ "Wire Protocol". The older ARQ layout uses the extension for the bandwidth token on call and CQ frames (ARQ). Current CALL2 adds the fields in section 4.3. Seeded carousel DATA and control payloads have their own headers in sections 4.5 and 4.6, so this byte layout must not be prepended to those payloads.

| Type | Use | Introduced |
|---|---|---|
| 0 | control (1.9.x ACK, DISCONNECT, KEEPALIVE and others) | ARQ |
| 1 | data (1.9.x stop-and-wait plane, removed in 1.9.18) | ARQ |
| 2 | CALL and ACCEPT of 1.9.x (refused by 1.9.20 and later) | ARQ |
| 3 | broadcast control | ARQ, BWF |
| 4 | broadcast data (legacy split format) | ARQ, BWF |
| 5 | CQ | ARQ |
| 6 | CALL2 and ACCEPT of "2.0" | 1.9.20 (CARQ) |
| 7 | station message (the HERMES beacon) | 1.9.23 (REL-1.9.23) |

**Frame check.** Each coded frame ends in a 16-bit CRC over the payload. In a session the sender XORs the CRC with a 16-bit seed, so every decoder in the station checks against that seed. The control decoder also accepts plain-CRC frames: CALL2, ACCEPT, DISCONNECT, broadcast and station frames. HARQ combining is off in a session because consecutive frames are different codewords. [D] CARQ "Session", CSPEC A3 and CSPEC A4.

- **Seed inputs:** the session id, the callee's 16-bit nonce from the ACCEPT (little-endian), the caller's callsign, a slash, the callee's callsign, upper-cased, and (added later) the session's piece size. A result of 0 becomes 0x5A5A. [D] CSPEC A4 and CSPEC R25. The order and byte layout of the final form, including the piece size, are not published (G-02).
- **CRC16 polynomial, initial value and bit order:** not published for the frame check (G-01). The callsign CRC in connect frames is called CRC16-CCITT (ARQ), and "little-endian" is stated. The exact callsign string that is hashed (case, SSID, padding) is not published (G-04).

**Block check (data plane).** Each block of application bytes ends in a 4-byte check: CRC-32 (IEEE, reflected, initial and final XOR 0xFFFFFFFF) over the seed (4 bytes, little-endian), the block id (8 bits), K (the number of data pieces) and the application bytes, stored little-endian. A block that fails the check, fails to decode, or arrives with two inconsistent size descriptions ends the session. [D] CSPEC section 3.1 and section 11, CARQ "Integrity". How the 16-bit seed becomes 4 bytes is not stated (G-02).

Sources: ARQ ("Wire Protocol", "Frame Types"), CARQ, CSPEC sections 3, 8, 11, REL-1.9.23. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/ARQ.md` (v1.9.23); Mercury `docs/RELEASE-1.9.23.md` (v1.9.23).

### 4.3 Connect frames

Both are 14 bytes on DATAC16 and padded to the 98-byte frame on MFSK. [D] CARQ "Wire formats".

| Byte | CALL2 | ACCEPT |
|---|---|---|
| 0 | packet type 6 (3 bits); extension: bandwidth token (2 bits), minor revision (2 bits, value 1), "sender transmits only MFSK" (1 bit) | same |
| 1 | 0, then session id (7 bits) | 1, then session id (7 bits) |
| 2 to 3 | CRC16 of the callsign being called | CRC16 of the caller's callsign |
| 4 to 5 | the caller's callsign, arithmetic-coded, running to byte 13 | CRC16 of the callsign the caller called (the callee's primary or secondary) |
| 6 to 7 | (callsign continues) | the callee's 16-bit session nonce |
| 8 | (callsign continues) | caller's start rung (3 bits, 0 to 6); "callee hears the caller below the control mode" (1 bit, and the rung is then the floor); 4 zero bits |
| 9 | (callsign continues) | SNR the callee read the CALL at, dB + 128 (0 means unknown) |
| 10 to 13 | (callsign continues) | zero |

Field order inside byte 0's extension and the bit order inside bytes 1 and 8 are listed most-significant first in the sources, matching the 1.9.x layout in CSPEC. [I] for the 2.0 fields.

- **Bandwidth token:** 0 reserved, 1 BW500, 2 BW2300, 3 BW2750. CALL advertises the caller's token. ACCEPT returns the negotiated one, the minimum of the two. [D] ARQ, TNC ("BW500"). S-CONN says the callee answers with the CALL's bandwidth. [C]
- **Acceptance rules:** a station takes only CALL2 with any 2.x minor revision, and drops anything else. An ACCEPT with a reserved bit set is dropped. The caller takes an ACCEPT only if its session id, its own callsign CRC and the called station's CRC all match (39 bits). [D] CARQ
- **Session id:** 7 bits, derived from the clock with the low bit set, so back-to-back sessions can share one. [D] S-TEAR. ARQ calls it random and one byte in the 1.9.x header; S-TEAR calls the DISCONNECT check 6 bits. [C, G-14]
- **Callee behaviour:** the callee answers every repeat of a session's CALL with the same ACCEPT (same rung, same nonce), because the caller may have decoded any of them. Another station's CALL is ignored while a session is being answered. [D] CARQ, CSPEC S1
- **Start rung and piece size:** the callee picks the caller's start rung from the SNR it read the CALL at. If the start rung is 0 (floor) or the callee hears the caller below the control mode, both ends cut 23-byte pieces. Otherwise 24. [D] CARQ "Blocks and pieces"
- **Minor-revision rule:** a station with the "MFSK only" bit set ignores minor 0, because minor 0 would put the peer's rounds on rungs it cannot send. A station of minor 1 reads 1.9.20's frames as minor 0 with the flag clear, and the two connect normally. [D] CARQ
- **CALL does not carry the callee's callsign in full.** The caller knows whom it called, and the CRC confirms it. [D] CARQ
- **Frames from 1.9.x (type 2):** logged as "another protocol version" and not answered. [D] CARQ, UP-1.9.18

Sources: CARQ "Wire formats", CSPEC sections 3.4, 14, 18, 19, ARQ, TNC, S-CONN, S-TEAR, UP-1.9.18. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/ARQ.md` (v1.9.23); Mercury `docs/TNC.md` (v1.9.23); Mercury `specs/connect/README.md` (v1.9.23); Mercury `specs/teardown/README.md` (v1.9.23); Mercury `docs/UPGRADING-TO-1.9.18.md` (v1.9.23).

### 4.4 Connect procedure

The caller (`CONNECT` from the host) works as follows. [D] unless marked.

1. Pick a session id and send CALL2 on DATAC16.
2. The first 2 CALLs go on DATAC16. After a host `RETRIES n` command the count of DATAC16 CALLs before the first MFSK CALL is `n / 2`, at least 2 and at most 4. Later CALLs alternate between the MFSK floor of the caller's own bandwidth and DATAC16. [D] TNC "RETRIES", CSPEC S0.
3. The gap between DATAC16 CALLs follows `CALLINT` (default 8 s, minimum 4 s). The MFSK CALL interval is 24 s in the model's source and 28 s in another passage [C, G-15]. A CALL is held while a decoder is on an arriving frame, and then until 5 s after that frame ends, so an ACCEPT decodes before the next CALL. [D] CARQ "Connect", S-CONN, TNC "CALLINT"
4. No new CALL is keyed after `arq:call_max_s` (default 300 s, 0 for no cap, at most 3600). [D] CSPEC section 21, REL-1.9.23
5. From the second CALL on, or on any MFSK CALL, the caller also listens on the floor, where an ACCEPT may come. [D] CARQ

The callee (`LISTEN ON`) works as follows.

1. A CALL2 whose called-callsign CRC matches one of the station's callsigns (or any, with `PUBLIC ON`) moves it to answering. The host sees `PENDING`. [D] TNC
2. After a guard of 0.7 s [D] CSPEC section 1.4 (pre-2.0 revision, [I] still current), it sends the ACCEPT on the carrier the CALL arrived on. Exception: a repeated CALL on DATAC16 means the caller heard no ACCEPT there, so the ACCEPT goes on the floor, unless the last one went there already. An MFSK-only callee answers only where its ACCEPT goes on the floor. [D] CARQ, REL-1.9.21
3. The callee sets the session seed when it sends the ACCEPT and connects when it hears the first seeded frame of any kind. [D] CSPEC S2, CSPEC S6
4. If ACCEPT retries run out, the callee keeps the seed and listens for 240 s before clearing it. [D] CSPEC S7

The caller connects when it decodes a matching ACCEPT. It records the callee's nonce, sets the seed, and sends its first round on the rung the ACCEPT named. The callee binds its payload decoder to that rung and keys nothing until it hears that round. [D] CARQ "Connect", CTURN section 4.

Mixed-version behaviour, measured by Mercury: a 1.9.17 caller sends 5 CALLs and gives up after about 1 minute against 1.9.18. A 1.9.18 caller sends 5 CALLs and gives up after about 2 minutes against 1.9.17, while the called station logs nothing. 1.9.19 and 1.9.20 refuse each other in 110 s. Nothing stays keyed. [D] UP-1.9.18, REL-1.9.19, REL-1.9.20.

Sources: CARQ, CSPEC S0 to CSPEC S7, S-CONN, TNC, UP-1.9.18, REL-1.9.19, REL-1.9.20, REL-1.9.21, REL-1.9.23. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/TNC.md` (v1.9.23); Mercury `specs/connect/README.md` (v1.9.23); Mercury `docs/UPGRADING-TO-1.9.18.md` (v1.9.23); Mercury `docs/RELEASE-1.9.19.md` (v1.9.23); Mercury `docs/RELEASE-1.9.20.md` (v1.9.23); Mercury `docs/RELEASE-1.9.21.md` (v1.9.23); Mercury `docs/RELEASE-1.9.23.md` (v1.9.23).

### 4.5 Data plane

The application's bytes are cut into blocks. Each block is Reed-Solomon coded over GF(256) as a systematic Cauchy code. Pieces 0 to K-1 are the data, and any K pieces rebuild the block. [D] CARQ "Blocks and pieces"

- **Limits:** a block has at most 96 data pieces. Up to 8 blocks are open at once. Piece indices run 0 to 255 and wrap, giving at least 160 distinct repair indices per block. Block ids are 8 bits internally and 4 bits (mod 16) on the wire. [D] CSPEC section 1.4, CSPEC I1 to CSPEC I4
- **Piece size:** 24 bytes, or 23 bytes for a session that starts on the floor (so an MFSK frame carries 4 pieces instead of 3). Fixed for the session. The field polynomial and Cauchy points are not published (G-10). [D] CARQ

**Historical DATA layout, not a complete v1.9.23 wire definition.** The next two tables describe the pre-timing-master baseline in CARQ and CSPEC 3.1 [D]. CTURN section 8 later requires a 3-bit slot index and a pattern-anchor flag in the DATA header. Their exact bit positions, and which older fields they replace, are not specified in the cited prose (G-11, C-1). In particular, do not use the start-rung hint or the reserved-zero assignment below as confirmed v1.9.23 fields. This gap must close before serialising current DATA frames. A DATA frame fills the payload mode's payload size.

| Bytes | Field |
|---|---|
| 0 | frames remaining after this one in the round (4 bits), "app data not yet cut into blocks" (1), start-rung hint for the peer's direction (3; value 7 invalid) |
| 1 | poll id answered (4), highest block id opened, mod 16 (4) |
| then, up to 8 segments | 4-byte header, then consecutive pieces |

**Historical segment header** (32 bits) [D] CSPEC 3.1, subject to the DATA-layout qualification above:

| Bits | Field |
|---|---|
| 31 to 28 | block id mod 16 |
| 27 to 21 | K - 1 (K at most 96) |
| 20 to 15 | piece count in this segment (1 to 63; 0 ends the list) |
| 14 to 7 | index of the first piece (later pieces use consecutive indices mod 256) |
| 6 to 2 | pad bytes in the last data piece (less than the piece size) |
| 1 | zero |
| 0 | flag indicating that the sender hears the peer below the control mode (first segment only) |

A segment header with K - 1 = 127 marks an inline poll that rides in the first frame of a round in place of a separate control frame. [D] CTURN section 8

Rules a sender must follow [D] CSPEC section 5.1 (pre-2.0 revision), CTURN section 8:

- Frame 0 of a round may begin with a short resend segment (at most 2 data pieces from the index the receiver reports waiting on). Other frames fill oldest block first and are topped up with further pieces of the oldest block.
- Repair is sized from the receiver's reported loss with a margin of 1.3 times loss plus 0.05.
- A floor round carries only the oldest block not yet stopped.
- A frame whose frame count a keydown on that rung cannot hold is corrupt and is dropped.

**Receiver rules** [D] CSPEC 5.3 and CSPEC P5 to CSPEC P7:

- A frame is accepted only on the rung the payload decoder is bound to. Another rung's burst is not decoded at all.
- A frame counts toward the round only if its poll id matches the current poll (at the floor any frame counts).
- Data pieces are handed to the application in order as soon as the contiguous prefix arrives, so a failed block check can end the session after those bytes were delivered. The stream is never silently wrong, because the session ends. [D] CSPEC section 11

Sources: CARQ, CSPEC sections 1.4, 3.1, 5.1, 5.3, 7, 11, CTURN section 8. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/CAROUSEL-TURNS.md` (v1.9.23).

### 4.6 Control frames

14 bytes on DATAC16. When the sender's peer reports it hears the sender below the control mode, the control frame goes on the MFSK floor instead, as a full 98-byte frame whose byte 0 is 0x07 and bytes 1 to 14 hold the control frame. 0x07 cannot be confused with a DATA frame because a start-rung hint of 7 is invalid. [D] CSPEC 3.2

Layout used by 1.9.20 and later [D] CARQ "Wire formats" (field order most-significant first [I]):

| Byte | Fields |
|---|---|
| 0 | type (2 bits: 0 POLL, 1 HANDOVER, 2 STATUS, 3 M_END), "has data" (1), poll id (4), "sender hears the peer below the control mode" (1) |
| 1 | rung asked for (3), frames asked for (4), SNR bit 0 |
| 2 | loss estimate (4 bits, round(loss x 15)), receive-window base block mod 16 (4) |
| 3 to 9 | 7 bytes of 8 x 7-bit "pieces still needed" for blocks base to base + 7 (127 means none seen) |
| 10 | first data piece missing from the base block (255 means none) |
| 11 | SNR bits 5 to 3 (3), handover round's rung (3), SNR bits 2 to 1 (2) |
| 12 | handover round's frame count (4), its poll id (4) |
| 13 | highest block opened, mod 16 (4), "data not yet cut" (1), plus a short-rung flag and a 2-bit backlog class (0 none, 1 up to 90 B, 2 up to 460 B, 3 more) [D] CSPEC 3.2 |

- **SNR field:** how the frame's sender hears the peer now, 6 bits, dB + 24 over -24 to +38 dB, 63 before any reading. The peer starts its own direction on the rung that SNR would start a session on. [D] CARQ, CSPEC section 18
- **Meaning by type:** POLL (receiver to sender) asks for `n` frames on a rung. `n = 0` means "acknowledgement only, this direction is done". STATUS (sender, polled, nothing to send) says what the sender holds. M_END is a POLL sent by a callee whose application has hung up, once it holds all of the caller's data. [D] CSPEC 3.2, CSPEC E10
- **Validation:** a frame with a rung or SNR-hint field of 7, or a needed-pieces value above 96, or a gap of 96 or more, is dropped. [D] CSPEC 3.2, CSPEC R19
- **Reserved bits** are not checked on receive in 1.9.x. [D] CSPEC 3.2. A coppa sender must send zeros.
- **HANDOVER type:** the 14-byte layout still lists handover fields (bytes 11 and 12), but CTURN removes handovers from the turn discipline. Which fields a 1.9.23 sender still fills is not clear. [C, G-11]

**ACK and BREAK patterns** (from the receiver) [D] CSPEC 3.3, section 11:

- At the floor an ACK means "keep going" (silence also means that). A BREAK means "the block your round carried is delivered", but it only stops the sender sending that block. Only a POLL retires a block. A finished direction at the floor is acknowledged by an ack-only POLL.
- Above the floor an ACK means the round came whole, and a BREAK means it lost no more than the last poll said. Neither retires a block.

Sources: CARQ, CSPEC sections 3.2, 3.3, 5.4, 8, 11, 18, CTURN section 8. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/CAROUSEL-TURNS.md` (v1.9.23).

### 4.7 Turn discipline

Mercury 1.9.18 replaced a handover design with one timing master, listed in REL-1.9.18 as "carousel: one timing master per session" (#351) and described in CTURN. CARQ's paragraph "The turn" and CSPEC section 5 still describe the old handover and a 30 s and 45 s turn quantum. Treat those as stale. [C-1]

Rules [D] CTURN sections 2 to 5:

- **The caller is the only station that keys on a timer.** The callee only answers, and roles never change whichever way data flows.
- **Callee slots.** The callee may key only in a slot anchored to the end of a caller keydown it decoded. The response slot starts `Tg` after that end. Continuation slots follow: slot i starts `i x P` after the response slot, with `P = r + A`, where `r` is the longest answer the caller's keydown allowed and `A` is an extra gap. Only a floor poll asking for more than one frame has continuation slots. The callee keys at most once per slot, and each callee keydown is at most `r` long.
- **Caller rule.** The caller may start a keydown only if the window from `G` before its start to `G` after its end misses every interval in which the callee could be on the air. Hearing the callee only changes what the caller sends, and lets it reuse a slot early. The exception is a slot in which the caller decoded the callee end to end.
- **Frame requirements.** Every frame states its position in its keydown (frames remaining), so a decoded frame reveals the keydown's end. A caller keydown states `r`. A callee keydown names the caller keydown (poll id) and slot (3 bits in the DATA header) that it answers. The modem timestamps each decode with the age of the frame's end, and the callee answers only if that age is at most `D`.
- **Assumptions.** Every decode occurs within `D` of the keydown's end. The two clocks agree within `G` over one slot pattern (50 ppm over 5 minutes is 15 ms). The caller uses true airtimes of the callee's possible keydowns.
- **Floor behaviour.** At the floor the callee continues in slots 1 to K (K = 2) while it hears nothing.
- **Disconnect.** The caller sends DISCONNECT clear of the callee's slots. The callee answers with an ACK pattern.
- **Idle.** The callee falls back to listening at the floor after 90 s of nothing from the caller. An unanswered idle poll is asked again 3 s after the caller decides. A request/response application answers within about 1 s, so the caller's ack-only completion waits up to 1 s for data to carry.
- **Watchdog.** Each end drops the session after 240 s without a seeded frame or pattern from the peer, with no DISCONNECT sent.

Values the documents give in real time: `Tg` is 0.9 s (the response gap), `G` is 0.7 s, the lead margin for a headed keydown is 0.3 s, and a keydown head of 110 ms and tail of 200 ms appear in CSPEC section 1.4. The seconds values for `D` (per rung), `A`, and the continuation counts other than K = 2 at the floor are not stated (G-11). The timed model proves no overlap and that the caller is never shut out, in small configurations (CTURN section 6, S-TURN). It does not cover pattern false alarms, which can still cause a collision (S-TURN).

Sources: CTURN, REL-1.9.18, CREP, S-TURN, CSPEC sections 1.4 and 5. Paths: Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/CAROUSEL-TURNS.md` (v1.9.23); Mercury `docs/CAROUSEL-REPORT.md` (v1.9.23); Mercury `specs/turns/README.md` (v1.9.23); Mercury `docs/RELEASE-1.9.18.md` (v1.9.23).

### 4.8 Rate ladder

The receiver of each direction chooses the rung and the frame count and sends both in its POLL. The sender keys only when polled. [D] CARQ "The receiver drives"

| Rung | Mode | Payload B | Airtime incl. sync (ms) | Pieces per frame | Gap between bursts (ms) | Frames per keydown cap | Block K target | Rung SNR floor (dB) | Start on this rung if SNR at least (dB) |
|---|---|---|---|---|---|---|---|---|---|
| 0 | MFSK floor | 98 | 13500 | 3 (4 with 23-byte pieces) | 200 | 2 | 15 | none | default |
| 1 | DATAC15 | 30 | 4400 | 1 | 100 | 3 | 14 | -7 | -3 |
| 2 | DATAC4 | 54 | 5800 | 2 | 100 | 3 | 22 | -6 | never a start rung |
| 3 | DATAC3 | 126 | 3820 | 5 | 100 | 7 | 80 | -1 | +1 |
| 4 | DATAC1 | 510 | 4810 | 21 | 100 | 6 | 96 | +3 | +8 |
| 5 | DATAC17 | 1180 | 7400 | 48 | 100 | 4 | 96 | +7 | +12 |
| 6 | QAM16C2 | 1213 | 3700 | 50 | 200 | 8 | 96 | +13 | +18 |

Source: CSPEC section 1.3, at the document's base revision. 1.9.20 changed the start rule to use an explicit SNR in every control frame, and the table's thresholds were not restated for it. Pieces per frame equal `(payload - 6) / 24` rounded down. [V] DATAC15 gives 1, DATAC4 2, DATAC3 5, DATAC1 21, DATAC17 48, QAM16C2 50. Control frames use DATAC16, 14 bytes, 3.74 s airtime. [D] CSPEC 1.3

- **Bandwidth cap.** BW500 allows rungs 0 to 3 (MFSK500 as its floor, then DATAC15, DATAC4, DATAC3). BW2300 and BW2750 allow rungs 0 to 6, and BW2750 has no extra rungs yet. Both ends derive the cap from the bandwidth the ACCEPT negotiated. [D] CARQ, TNC "BW500"
- **Link adaptation (receiver-local policy).** The receiver ranks rungs by measured goodput (raw rate times delivered fraction), probes only rungs that could beat the best, limits a rung's round to one frame more than it recently delivered, and declares a rung dead after 4 lost frames in a row (2 near its SNR floor, or after two silent rounds in a row). A dead rung is re-probed after 60 s, doubling to 8 minutes. The policy is local, so a coppa receiver may use its own, provided it only names rungs both ends hold. [D] CARQ, CSPEC section 5.10, section 21
- **Rung agreement.** Both ends must fall back to the same rung after silence. After silence the caller returns to the last rung it confirmed, and the callee to its previous rung. A round sent on a rung the peer is not bound to costs a round and counts as loss. A model checks this at small bounds (agreement, at most one wasted round per loss) but excludes the floor rung and the "deaf to control" decision. [D] S-RUNG, CTURN section 8
- **Control-mode deafness.** A station that hears its peer below DATAC16 reports it, and control frames to it then go on the MFSK floor. At the base revision the SNR average turns it on below -7.5 dB and off at -6.0 dB, and an MFSK frame's own reading alone can turn it on. [D] CSPEC section 2.2, section 19
- **Silence rule (1.9.20).** A keydown that ends with nothing of the peer decoded for 150 s (`arq:silence_fallback_s`, range 110 to 600 s) takes both directions below the control mode. The callee falls back at 60 percent of that, and 90 s after two unanswered asks by the caller. [D] CSPEC sections 18 and 19, S-RUNG, CTURN section 8 [C, G-12]

Sources: CARQ, CSPEC sections 1.3, 2.2, 5.10, 18, 19, S-RUNG, CTURN, TNC. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/CAROUSEL-TURNS.md` (v1.9.23); Mercury `docs/TNC.md` (v1.9.23); Mercury `specs/rungs/README.md` (v1.9.23).

### 4.9 Teardown

- The host sees `DISCONNECTED` at once when it hangs up. The air side then delivers queued and unconfirmed data for up to three exchanges at the session's rung (minutes at the floor), and stops waiting after 120 s with nothing heard from the peer. A second host `DISCONNECT` or a `CONNECT` ends the drain at once and drops undelivered data. [D] TNC "DISCONNECT", S-TEAR
- The DISCONNECT frame is plain-CRC and bound to the session id. A DISCONNECT with a matching session id ends a session, which S-TEAR says happens about 1 time in 64 for a foreign station. [D] CSPEC 3.4, S-TEAR. Its byte layout is not published (G-06).
- The reply to a DISCONNECT is an ACK pattern keyed by the session seed (0.64 s, against 3.7 s for a DISCONNECT frame and 13.5 s at the floor). Only a station whose own DISCONNECT is out accepts it as the answer. [D] CTURN section 8, S-TEAR
- **Caller hung up first:** the caller sends DISCONNECT clear of the callee's slots, repeating up to the retry count while nothing answers. With nothing heard from the peer for 120 s it sends one DISCONNECT, and the callee ends at once. [D] CTURN section 8, S-TEAR
- **Callee's application hangs up:** the callee marks its POLLs as M_END once it holds all of the caller's data. A caller that has everything delivered both ways ends the session. If not drained, it ignores M_END, and the callee's own DISCONNECT ends the session later. A callee that owes an acknowledgement of the caller's last data answers with M_END instead of a DISCONNECT. A caller that heard M_END and then sees the callee silent for as long as the callee's own drain would last (at most 120 s) sends one DISCONNECT. [D] S-TEAR, CSPEC section 21
- **Crossed DISCONNECTs:** each end that hears the other's DISCONNECT ends, and an ending end replies only if its own DISCONNECT had not gone out. A callee waiting for the answer to its DISCONNECT takes a CALL from any callsign, because the application may use a different `MYCALL`. [D] S-TEAR
- **Lost peer:** a connected end that hears nothing for 240 s drops the session locally. A callee waits 180 s for the answer to its own DISCONNECT. [D] S-TEAR, CSPEC E2
- A DISCONNECT's deferral counts its 10 s cap from the end of the station's own keydown (a 32 s separate cap applies while keyed). [D] CSPEC section 21 (#500)

Sources: TNC, S-TEAR, CTURN section 8, CSPEC sections 3.4, 5.12, 21. Paths: Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/CAROUSEL-TURNS.md` (v1.9.23); Mercury `docs/TNC.md` (v1.9.23); Mercury `specs/teardown/README.md` (v1.9.23).

### 4.10 Other on-air frames

- **CQ (type 5).** 14 bytes on DATAC16: byte 0 is the packet type and bandwidth token, bytes 1 to 13 are the source callsign, arithmetic-coded. A decoded CQ gives the host `CQFRAME <call> <bw>`. The trigger and repetition are not documented. [D] ARQ "CQ compact frame", TNC
- **Station message (type 7), 1.9.23.** The HERMES position beacon. It is sent on the mode the host selects (`BEACONMODE`, default DATAC16; MFSK or MFSK500), after the channel is heard clear, and delivered from any decoder. Older releases drop it. The layout is not published (G-06). [D] REL-1.9.23, CSPEC section 21, TNC
- **Broadcast plane (types 3 and 4).** One-to-many, no ARQ, a RaptorQ fountain over a file bundle. Out of scope for ARQ interoperability. Facts for a later ticket: transport is KISS over TCP on port 8100; the joint frame is type 3 with the extension holding a session id of 1 to 31; its body is a reduced RFC 6330 object descriptor (transfer length 24 bits, symbol size minus 1 in 16 bits, source blocks minus 1 in 8 bits, sub-blocks minus 1 in 16 bits) plus a 3-byte tag and the symbols; the symbol size is 41 bytes. [D] BWF, BFILE. The two documents disagree on symbols per frame. [C]

Sources: ARQ, TNC, REL-1.9.23, CSPEC section 21, BWF, BFILE. Paths: Mercury `docs/CAROUSEL-SPEC.md` (v1.9.23); Mercury `docs/ARQ.md` (v1.9.23); Mercury `docs/BROADCAST-WIRE-FORMAT.md` (v1.9.23); Mercury `docs/BROADCAST-FILE.md` (v1.9.23); Mercury `docs/TNC.md` (v1.9.23); Mercury `docs/RELEASE-1.9.23.md` (v1.9.23).

### 4.11 Version marker

Mercury has no single named air-protocol version. What exists on the wire [D] CARQ, REL-1.9.21, REL-1.9.23, README:

- **Packet type 6 (CALL2)** since 1.9.20, so 1.9.20 and later refuse older CALLs cleanly and nothing stays keyed.
- **Minor revision:** a 2-bit field in the connect extension, currently 1. Older 2.x stations read it and ignore it. A station with the MFSK-only bit set ignores minor 0.
- **History:** from 1.9.18 to 1.9.20 the CALL frame carried a 2-bit protocol version, with 1 = 1.9.18, 2 = 1.9.19 and 3 = a trunk build with its own BW500 floor. 1.9.20 replaced it with the packet type.
- **Forward rules that exist:** unknown broadcast and station packet types are dropped. Unknown minor-revision bits are ignored. An ACCEPT with a reserved bit set is dropped.
- **No policy** says whether future wire changes will keep the 2.x packet type, bump the minor revision, or appear in a patch release. Mercury's own breaks happened in patch-numbered releases and were announced in bold in the release notes (REL-1.9.18, REL-1.9.19, REL-1.9.20).

Sources: REL-1.9.18 to REL-1.9.23, UP-1.9.18, CARQ, README. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `README.md` (v1.9.23); Mercury `docs/UPGRADING-TO-1.9.18.md` (v1.9.23); Mercury `docs/RELEASE-1.9.18.md` (v1.9.23); Mercury `docs/RELEASE-1.9.23.md` (v1.9.23).

### 4.12 Mercury tooling for conformance testing

- **Ports:** control 8300, data 8301, broadcast KISS 8100. [D] TNC, README
- **Headless:** the `mercury` binary is the engine. Audio backends `fifo` (raw signed 32-bit little-endian samples at 8 kHz via `-i` and `-o`), `sock` (framed, virtual-clock lockstep), `rtp`, `shm`, `null` and the sound-card backends. `-r` runs a receive test, `-t` a transmit test, `-d` traces decoded connect frames. [D] README
- **Monitor mode** (`MONITOR ON`) decodes every frame heard except carousel sessions, which carry the seed. [D] REL-1.9.18, TNC
- **Harness:** a Go two-daemon test runs two real daemons over FIFO or socket audio through codec2's `ch` channel tool or Mercury's Watterson model, and can run two different builds against each other. [D] DECK, CARQ "Tests", REL-1.9.22
- **Not published:** test vectors, a conformance suite for third parties, or WAV-file decode of a recording through the public interface. [G-16]

Sources: README, TNC, REL-1.9.18, REL-1.9.22, DECK, CARQ. Paths: Mercury `docs/CAROUSEL-ARQ.md` (v1.9.23); Mercury `docs/TNC.md` (v1.9.23); Mercury `README.md` (v1.9.23); Mercury `docs/presentations/2026-10-mercury/mercury.tex` (v1.9.23); Mercury `docs/RELEASE-1.9.18.md` (v1.9.23); Mercury `docs/RELEASE-1.9.22.md` (v1.9.23).

## 5. Gaps

Sources: the versioned documents cited in sections 3 and 4. These are documentation gaps identified by comparison, not claims that code was inspected.

Eight gaps block a bit-exact connect, including identity checks, field packing and timing. "Only in" identifies a route to the answer, not a verified source location.

| ID | Gap | Blocks | Only in | Blocks a connect |
|---|---|---|---|---|
| G-01 | Frame-check CRC16: polynomial, initial value, bit order, byte order, and exactly which bytes it covers | every frame | Mercury or codec2 source, or measurement | yes |
| G-02 | Seed construction: byte order and layout of the hashed inputs including the piece size, and how a 16-bit seed becomes the 4 bytes used in the block check | every session frame | Mercury source, or measurement | yes |
| G-03 | Arithmetic callsign coder: alphabet, model, SSID handling, terminator, padding for the 10-byte (CALL2) and 13-byte (CQ) fields | connecting as caller, CQ | Mercury source, or measurement | yes |
| G-04 | Exact string hashed for the callsign CRC16 fields (case, SSID, padding) | connecting | Mercury source, or measurement | yes |
| G-05 | Bit order inside the 2.0 control and connect bytes (listed most-significant first in the docs, verified only against the 1.9.x layout) | control frames | Mercury source | yes |
| G-06 | DISCONNECT frame layout, current CQ details, station-frame (type 7) layout | teardown, beacons | Mercury source | no |
| G-07 | MFSK: Gray and hop mapping, preamble and postamble tone sequences and lengths, LDPC matrix, interleaver seed and generator, CRC width and polynomial, bit packing, MFSK500 tone placement, exact timing | all floor traffic | Mercury source, or measurement | yes |
| G-08 | OFDM modes: pilot values, UW bit patterns, preamble and postamble waveforms, carrier index map, bit-to-symbol mapping, scrambling and interleaving, LDPC matrices, DATAC15, DATAC16 and DATAC17 carrier count and timing (here only derived) | every OFDM rung | codec2 and Mercury source, or measurement | yes |
| G-09 | Tone patterns: global ACK and BREAK lists, the 12 NAV lists, the session-list derivation, the pattern length at BW500, whether NAV is sent in a session | floor signalling | Mercury source, or measurement | no |
| G-10 | Reed-Solomon construction: field polynomial, Cauchy points, piece ordering | the data plane | Mercury source | no |
| G-11 | Real-time slot values: `D` per rung, `A`, continuation counts, PTT lead and tail, frames-per-keydown caps in the current release, whether HANDOVER still appears, and exact placement of the DATA slot index and pattern-anchor flag that supersede the historical header fields | turn discipline and current DATA serialisation | Mercury source, or measurement | no |
| G-12 | Silence and fallback timers: the relation of the 90 s and 60-percent rules, per-mode SNR calibration lines, and whether the thresholds in 4.8 still hold in 1.9.23 | liveness only (local policy otherwise) | Mercury source | no |
| G-13 | Minor-revision field rules beyond "ignore unknown bits", reserved-bit list, behaviour when a future minor value appears | forward compatibility | Mercury maintainers | no |
| G-14 | Session id width and origin (7, 6 or 8 bits), DISCONNECT retry counts, slots and drain constants | teardown | Mercury source | no |
| G-15 | CALL timings: the MFSK CALL interval (24 or 28 s), the ACCEPT window, ACCEPT repeat count, the default total CALL count (5 observed) | connect timing | Mercury source | yes |
| G-16 | No over-the-air validation of cross-implementation behaviour, no published test vectors, no way to feed a recording to Mercury's decoder through a documented interface | verification | Mercury maintainers | no |
| G-17 | Which codec2 revision Mercury vendors and what it changed (documented changes: burst acquisition, a HARQ hook, a TX clip setter) | OFDM waveform fidelity | Mercury source | no |
| G-18 | Encryption: S-CONN describes encryption offer and answer bits in the handshake, but 1.9.20 to 1.9.23 do not ship encryption and CARQ's ACCEPT has only reserved bits at those positions. A coppa sender must send zeros, and any set reserved bit gets the ACCEPT dropped. The planned Noise_KK design would change the wire (DECK, REL-1.9.20) | forward compatibility | Mercury maintainers | no |

Closing G-01 to G-10 needs source reading by a person the owner authorises, or the black-box route in section 8.8. Neither route was started for this document.

## 6. Conflicts between the cited documents

Sources: source keys resolve to the paths and versions in section 1. Each row names both sides of the conflict.

| ID | Conflict | How the spec treats it |
|---|---|---|
| C-1 | CARQ ("The turn", "Only the receiver keeps a round timer") and CSPEC section 5 describe a handover with a 30 s and 45 s turn quantum. CTURN, REL-1.9.18 and CREP say #351 replaced it with one timing master. CSPEC sections 18 to 21 use terms from the new design. | The timing-master rules (4.7) are treated as shipped. The old text is stale. The DATA tables in 4.5 are historical; G-11 withholds current slot/anchor bit positions. |
| C-2 | CARQ says NAV headers open every keydown below 0 dB. CTURN says they are not sent in a session. | Unresolved (G-09). |
| C-3 | DATAC16 bandwidth: 187.5 Hz (BFILE) against 2000 Hz (ACQW). | 187.5 Hz, consistent with 3 carriers. |
| C-4 | DATAC16 frame time: 3.08 s (MODES table) against 3.74 s (CSPEC, TNC). | Both: frame 3.08 s plus 0.66 s sync equals 3.74 s airtime. [V] |
| C-5 | BW500 floor airtime: 26.2 s (CARQ, CTURN), 27 s (TNC, REL-1.9.22), 27.6 s (S-RUNG). | 26.2 s frame, with 27 s as the keydown budget. [I] |
| C-6 | MFSK frame time: 13.1, 13.5 s in different sections of MFSKP. | about 13.5 s. |
| C-7 | MFSK CALL interval: 24 s against 28 s (S-CONN). | G-15. |
| C-8 | Bandwidth token negotiation: minimum of both (ARQ, TNC) against the CALL's bandwidth (S-CONN). | The minimum, from the two documents that state the rule. |
| C-9 | Session id: 7 bits (CARQ), 6 bits (S-TEAR DISCONNECT check), one random byte in the 1.9.x header (ARQ), clock-derived (S-TEAR). | G-14. |
| C-10 | `ARQ.md` labels its 14-byte CALL and ACCEPT layout "1.9.x". CARQ gives the 2.0 layout. | CARQ is used for CALL2 and ACCEPT. |
| C-11 | QAM16C2 and DATAC17 sizes: 1213 and 1180 B (MODES, BFILE) against 1215 and 1182 B (CLIP, which counts the CRC). Duration 3.10, 3.2, 3.30 s. | 1213 and 1180 are payloads. Durations flagged. |
| C-12 | Broadcast symbols per frame: one (BWF sections 3 and 7) against many (BFILE). | Out of scope. |
| C-13 | The `frames asked for` field is 4 bits, but a keydown cap of 17 frames appears in CSPEC. | The per-rung caps in 4.8 stay at 8 or fewer. |
| C-14 | codec2's `README_data.md` lists no DATAC15, DATAC16, DATAC17 (Mercury-defined). MODES names QAM16C2 as ported from the Rhizomatica fork, DECK lists it as codec2's. | Treated as ported. |

## 7. Approval needed

Sources: Mercury v1.9.23 `docs/MODES.md`, `docs/MFSK-PORT.md` and `docs/CAROUSEL-ARQ.md`; codec2 `README_data.md` and `doc/modem_codec_frame_design.ods` as fetched 2026-11-10. Source keys and licences resolve in section 1. Dimensions are facts or labelled inferences; no matrix or sequence values are included.

The ticket requires every table larger than a handful of values to be listed by name and size, and left out of the spec until the repo owner approves. None of these appear in sections 3 and 4. Status of every item: **not included, awaiting the owner's approve or reject.**

| ID | Table or sequence | Size | Source and licence | Needed for interop | Can it be restated as a few parameters? |
|---|---|---|---|---|---|
| A-1 | LDPC parity-check matrices of the stock OFDM modes: (256,128), (8192,4096), (2048,1024), (1472,448), (384,128), (112,56) | H is (n-k) x n: 128x256, 4096x8192, 1024x2048, 1024x1472, 256x384, 56x112. Entry counts not stated in the documents. | codec2, LGPL-2.1 | yes, per mode | no |
| A-2 | the (768, 256) rate-1/3 LDPC code that `docs/MODES.md` lists for DATAC15 and DATAC16 | 512x768 | name from MODES. Origin unclear: codec2 LGPL-2.1 or Mercury GPL-3.0 | yes, DATAC15 and DATAC16 | no |
| A-3 | LDPC matrix (16200, 9720) rate 3/5 for QAM16C2 and DATAC17 (shortened to (15936, 9456)) | 6480x16200 | C2ODS says "DVBS2 code". Parameters match ETSI EN 302 307-1 V1.4.1 Table 5b [D], a copyrighted public standard. Exact matrix identity remains unverified [I]; otherwise codec2, LGPL-2.1. | yes, rungs 5 and 6 | no. The standard's table is compact but still a table. |
| A-4 | MFSK LDPC code N=1600, K=800 (rate 8/16, IRA) | 800x1600 | Mercury, GPL-3.0 (ported from Mercury 1.x) | yes, floor | no |
| A-5 | MFSK interleaver permutation | one permutation of 1600 positions | Mercury, GPL-3.0 | yes, floor | yes, if the PRNG, seed and shuffle order are published. They are not. |
| A-6 | MFSK symbol mapping: Gray code and tone-hop sequence | 32-entry map, hop sequence length not stated | Mercury, GPL-3.0 | yes, floor | maybe. Gray is a rule, the hop sequence is not. |
| A-7 | MFSK preamble and postamble tone sequences | approximately 17 combined symbols inferred from airtime differences [I]; exact lengths unknown | Mercury, GPL-3.0 | yes, floor | no |
| A-8 | Tone-pattern lists: global ACK and BREAK lists, 12 NAV class lists, and the session-list generator | 2 x 8 + 12 x 8 = 112 tone values [V], plus a derivation | Mercury, GPL-3.0 | yes, floor signalling | the 112 values: no. The generator: yes, if published. |
| A-9 | OFDM unique-word bit patterns, pilot sequences and preamble and postamble definitions for 10 modes | UW from 8 to 42 symbols per mode (DATAC15 48 bits, DATAC16 32 bits), 168 bits derived for DATAC17. Pilot and preamble sizes not stated | codec2, LGPL-2.1, and Mercury for DATAC15 to DATAC17 | yes, all OFDM rungs | no |
| A-10 | Arithmetic-coder model for callsigns | alphabet and probability table, size not stated | Mercury, GPL-3.0 | yes, connect and CQ | no |
| A-11 | RaptorQ constant tables (RFC 6330: V0 to V3, degree distribution, systematic index) | RFC 6330 tables, several hundred entries | IETF RFC 6330, a public standard | broadcast plane only | no. Out of scope for this effort. |
| A-12 | Additional unpublished items needing an owner call: the Reed-Solomon construction (a few parameters, not a table), the CRC16 polynomials (a few values), per-mode SNR calibration lines (7 pairs, receiver-local) | a handful of values each | Mercury, GPL-3.0 | RS and CRC16: yes. Calibration: no. | yes |

Licensing effect worth stating for the owner: coppa is MIT OR Apache-2.0, and `deny.toml` does not allow LGPL or GPL dependencies (`docs/DECISIONS/2026-11-09-clean-room-and-licensing-policy.md` decision 6). Approval is not a relicensing grant. Copying protected table expression may impose the source licence; numerical facts and standard-derived constructions require separate provenance review. Decision 5 already requires the owner's approval and the source's copyright notice. This specification grants no licence permission.

## 8. Go/no-go assessment

Sources: coppa files in section 8.1, Mercury v1.9.23 release notes and the GitHub release timestamps through 2026-11-09. Component and effort ranges below are engineering estimates [I], not measured implementation results.

### 8.1 What coppa has today

Measured on 2026-11-11 at coppa `28d4c9a7fe64e71b31891ffafafeaf5f07318517`. The branch fast-forwarded from the dispatch base `d04f6f3`; no merge commit was needed.

| Area | Fact | Evidence |
|---|---|---|
| Sample rate | Every profile runs at 48 kHz, FFT 960, 50 Hz carrier spacing. `coppa_engine_new_with` rejects any rate other than 0 or 48000. | `crates/coppa-codec/src/ofdm/mod.rs`, `docs/LIMITATIONS.md` |
| Waveform abstraction | `CoppaProfile` is a plain struct, not a trait or enum. `CoppaTransceiver`, `StreamingReceiver` and `SyncDetector` take that concrete struct. Nothing abstracts over a waveform family. | `crates/coppa-codec/src/traits.rs`, `crates/coppa-protocol/src/modem/transceiver.rs` |
| MFSK | None. `rg -il mfsk --glob '*.rs'` finds no file. | ran |
| LDPC | One NR BG2 mother code (Zc = 176, 1944-bit coded block) plus an unused 802.11 set. The decoder builds from a const table and has no public constructor for an arbitrary parity-check matrix. | `crates/coppa-protocol/src/fec/ldpc/decoder.rs`, `codes.rs` |
| Erasure codes | `OuterRs12x6`, a fixed (12,6) erasure code over GF(256) with polynomial 0x11d, plus two Reed-Solomon codes for IL2P and FX.25. No RaptorQ, no variable-K systematic Cauchy. | `crates/coppa-protocol/src/fec/outer_rs.rs` |
| ARQ and session | Selective-repeat ARQ, window 1 in connected sessions, a connect handshake with `Disconnect` and no reply. There are no polls and no carousel. Callsigns pack 8 characters into 6 bytes. | `crates/coppa-protocol/src/session.rs`, `crates/coppa-protocol/src/arq.rs`, `crates/coppa-protocol/src/mac.rs` |
| Daemon | `coppad` holds one concrete `CoppaCore`. `event_loop.rs` is 25,984 lines (`wc -l`). `coppad --tnc` is the precedent for a different waveform on a separate code path. | ran `wc -l`, `cli.rs` |
| Host | VARA-style `BW500`, `BW2300` and `BW2750` return `OK` as "accepted, not implemented". | `event_loop.rs` |
| Resampler | `coppa-audio` has a `rubato` resampler, declared and compiled. `ARCHITECTURE.md` and `wiki/pages/band-conventions.md` say it is not. | `crates/coppa-audio/src/lib.rs` |
| Test vectors | 42 committed WAVs, generated by coppa's own bench example. Their prior research test run passed; this assembly does not claim a new dedicated vector run. | ran `cargo test -p coppa-protocol --test golden_vectors`: 3 passed, 35.63 s |
| External oracles | Direwolf, Pat and Reticulum harnesses exist for AFSK, IL2P, FX.25 and host interfaces, none in CI. None exists for an HF OFDM modem. | `tools/`, `tests/` |
| Public API gate | `cargo-semver-checks` guards 11 library packages. `EngineConfig`, `CoppaProfile`, `LinkCapabilities` and others are not `#[non_exhaustive]`, so adding a field is a breaking change. | `docs/DECISIONS/2026-11-02-public-rust-api-semver-gate.md` |
| Over-the-air evidence | None for coppa. | `docs/reviews/2026-09-05-competitive-landscape.md` |
| Policy | Decision 2 bars RF compatibility with proprietary modems and allows open, publicly specified modes. Mercury is GPL-3.0, so decision 2 does not bar it, and decision 4 bars reading its source. | `docs/DECISIONS/2026-11-09-clean-room-and-licensing-policy.md` (read in full) |

Prior research baseline at `d04f6f3`: non-test source size (approximate; physical lines, awk heuristic for inline tests): coppa-dsp 915, coppa-codec 9,021, coppa-protocol 10,257, coppa-ml 1,090, coppa-engine 2,528, coppa-host 4,399, coppa-ffi 1,744, coppa-cli 4,096, coppa-daemon 14,729, coppa-bench 3,386. Total about 52,000 lines. Whole `src/` directories: dsp 1,552, codec 14,920, protocol 20,209, ml 2,052, engine 5,236, host 10,068, ffi 3,655, cli 7,750, daemon 39,359, bench 6,441, channel 1,182, audio 4,687, radio 3,533. Whole-directory totals were remeasured on 2026-11-11.

### 8.2 Crates touched

| Crate | Change | Public API effect |
|---|---|---|
| New crate (suggest `coppa-mercury`) | Everything new in 8.3. Keeping the new modem separate limits changes to semver-gated libraries. | none to existing crates |
| `coppa-dsp` | Possibly small additive primitives (tone correlator, filters). | additive only |
| `coppa-codec` | None if the waveforms live in the new crate. | none |
| `coppa-protocol` | None if the new crate carries its own LDPC and block layer. Reusing `fec/` would need new constructors. | additive only |
| `coppa-engine` | A new session-driver boundary so `coppad` can host a second personality. Adding fields to existing public structs is a break, so add new types. | additive only if done carefully |
| `coppa-daemon` | A new mode or binary, following `coppad --tnc`. Not a change inside the 25,984-line event loop. | none |
| `coppa-host` | Wire `BW500`, `BW2300` and `BW2750` to the ladder cap, and map session events. | small |
| `coppa-cli`, `coppa-bench`, `coppa-channel` | Encode and decode subcommands, vector tests, 8 kHz channel runs through the resampler. | none |
| `coppa-ffi`, `coppa-ml`, `coppa-radio`, `coppa-audio` | None. | none |

Estimate: one new crate; product changes in coppa-engine, coppa-daemon and coppa-host; tools and tests in coppa-cli, coppa-bench and coppa-channel; possibly small additions to coppa-dsp and coppa-protocol. Six existing crates change, or eight with the optional changes. Five of the existing 13 remain untouched in the widest case.

### 8.3 New components needed

| Component | Why it is new | Rough size, non-test Rust |
|---|---|---|
| codec2-style OFDM modem at 8 kHz, ten modes | Different symbol rate, pilot structure, UW, preamble and postamble from the Coppa stack | 4,000 to 6,000 lines |
| 32-tone MFSK modem and a streaming tone-pattern detector | No MFSK exists in coppa. Needs a matched-filter preamble search, +/-100 Hz frequency search, and session-keyed pattern lists | 2,500 to 3,500 |
| Generic LDPC codec for arbitrary parity-check matrices, 112 to 16,200 bits | The decoder is tied to NR BG2 | 1,000 to 1,800, plus the approved tables |
| Block layer: variable-K Cauchy Reed-Solomon, seeded CRC16, CRC-32 block check, segment framing | The existing RS is fixed (12,6) | 800 to 1,200 |
| Session layer: CALL2, ACCEPT, CQ, DISCONNECT, station frames, callsign coder, connect FSM over three carriers, timing-master slot log, rung agreement, silence fallbacks, teardown with drain and M_END, watchdogs | A new protocol. Mercury's own specification for it is more than 100 KB of prose | 5,000 to 8,000 |
| Integration: engine boundary, daemon mode, host mapping | | 1,500 to 2,500 |
| Verification: vectors, loopback against Mercury 1.9.23 binaries as a black-box oracle, 8 kHz channel runs | That is the third ticket | about 1:1 with the code |

### 8.4 Size of the work

[I] Estimate: 14,000 to 23,000 lines of new non-test Rust and 7 to 10 engineer-months for everything. Of that, the smallest useful slice (connect on DATAC16 and MFSK, the floor rung, carousel at the floor, DISCONNECT) is about 9,000 to 12,000 lines and 5 to 7 engineer-months. The full ladder and integration complete the total above; their incremental size is uncertain. The time estimate adds 2 to 3 engineer-months beyond the smallest slice. Each wire change in Mercury then costs about 1 to 2 weeks to follow. The full work is about 25 to 45 percent of coppa's current 52,000-line non-test code. By the repo's own sizing it is the largest single feature in the project, and it spans several quarters for one person.

These are engineering judgement, scaled from coppa's own component sizes (for example `session.rs` 1,435 lines, `arq.rs` 1,903, the LDPC decoder 1,538, `coppa_modem.rs` 2,826). These are estimates of future work, not measurements. The range excludes OTA tuning, which Mercury itself still does on air (CREP, REL-1.9.20 to REL-1.9.23), and it excludes the time to obtain the unpublished constants.

### 8.5 Churn risk

Computed from the GitHub release timestamps (ran a short `python3 -I` script over the release JSON):

- 24 `v1.9.x` tags, including two pre-releases, from 2026-03-07 to 2026-11-09 (247 days). Intervals in days: median 8.0, mean 10.8, minimum 0.3, maximum 49.9. Stable tags only: 22, median 8.0, mean 11.8.
- 1.9.18 to 1.9.23: 6 releases in 28 days, with 3 breaking the air protocol in the first 10 days and 2 adding frames afterwards. Since 1.9.20 (2026-10-22), 18 days have passed with no break.
- The documents are silent about wire changes for 1.9.2 to 1.9.17, so the break rate before October is unknown.
- Mercury's own roadmap items touch the wire: optional Noise_KK encryption, a wider mode for BW2750, a parked two-stream MFSK rung, and a QAM16C2 variant for peak-limited radios (REL-1.9.18 to REL-1.9.23, DECK, CREP).
- The failure mode of a mismatch is silent: the caller sends several CALLs, then gives up. An operator sees "no connect", not "wrong version" (UP-1.9.18, REL-1.9.19, REL-1.9.20).

Reading: a coppa implementation pinned to the 1.9.20 to 1.9.23 generation would work until the next wire change. If wire changes keep arriving at the October rate, coppa would be behind within weeks of shipping. Mercury's two additive releases show it can extend the wire without a break, which is the behaviour a shared profile would formalise.

### 8.6 Recommendation

**Propose a shared interop profile to Mercury's maintainers (option 2 of 3).** Keep the implementation ticket in Backlog. Do not start implementation until the conditions in 8.7 hold. If Mercury's maintainers do not answer within 60 days of the owner filing the requests in section 9, or decline, choose "do not pursue", and close the implementation ticket and the verification ticket.

| Option | Verdict | Reason |
|---|---|---|
| Implement now | no | 18 gaps, eight connect blockers and no stable target |
| Propose a shared interop profile | recommended | Requests may secure a version marker, vectors and table permissions |
| Do not pursue | default after 60 days without an answer, or a refusal | Bounds the effort spent waiting on missing prerequisites |

1. **The spec cannot reach bit-exactness from public documents.** 18 gaps and 12 approval items remain. Eight gaps block a bit-exact connect. Closing them needs source reading, which policy decision 4 bars for agents and implementers, or measurement of a real transmitter.
2. **The target moves fast and has no compatibility promise.** Median 8 days between releases, three air-protocol breaks in 10 days, silent failure on mismatch. A fixed implementation would chase it.
3. **The work is the largest in the project.** 14,000 to 23,000 new lines and several engineer-quarters, in a codebase with 52,000 non-test lines, a fixed 48 kHz waveform, no MFSK and no generic LDPC.
4. **coppa has no over-the-air evidence of its own yet.** Interoperating with a modem Mercury tunes on real radios would compare coppa's untested receiver against a field-tested one.
5. **Maintainers are reachable and their documents are written for re-implementation.** CSPEC says it is "the reference for reviewing it, modelling it and re-implementing it". They publish release notes, channel models and formal specs, and accept outside contributions (a channel-model PR from an outside contributor, field reports credited by callsign). A request costs a few hours. It may deliver the version marker, the vectors and the table permissions that would make the work cheap and stable.
6. **Policy supports it, with one amendment the owner should expect.** Decision 2 allows open, publicly specified modes, but its context line says both ends of an OFDM link run coppa. Mercury interoperability would make that line untrue for one waveform family, so the owner should amend decisions 1 and 2 before implementation starts (add to C3 below). Decision 2 bars RF compatibility only with proprietary modems, and Mercury is open and documents its protocol. Decision 4 allows running an unmodified Mercury binary as a black-box oracle with provenance recorded, which is how the verification ticket would work.

- **Implement now.** It fails 8.6 reasons 1 to 4. Even with unlimited effort the implementer lacks the constants, and there is no stable target.
- **Do not pursue now.** It gives up a cheap request that may remove reasons 1 and 2. It also leaves coppa with the only open-modem peer outside its interoperability story. The 60-day default makes this the outcome if the request goes nowhere.

### 8.7 Conditions that would change the recommendation to implement

Any implementation ticket should wait until all of these are true:

- C1. Mercury publishes an air-protocol version marker and a statement of what stays compatible within a generation (feature request FR-1).
- C2. Mercury publishes conformance vectors for at least DATAC16, MFSK and one payload rung, or a documented offline encode and decode interface (FR-2, FR-3).
- C3. The owner decides each approval item in section 7, at minimum A-1 to A-10 for the slice being built, and amends the clean-room policy's statement that both ends of an OFDM link run coppa.
- C4. The gaps G-01 to G-10 and the current DATA slot/anchor bit placement in G-11 are closed by a route the owner authorises (8.8), and the result is merged into `docs/interop/mercury-air-protocol.md`.
- C5. coppa has its own first over-the-air baseline, so interoperability is a second variable and not the first.

### 8.8 Routes to close the gaps

- **Route A, a person reads.** The ticket gives its author this permission. The owner records it in a `docs/DECISIONS/` entry before it starts. The person writes only the missing constants and rules into the spec, and no one who implements reads the source. This is the ticket's own process.
- **Route B, black-box measurement.** Decision 4 allows running an unmodified Mercury 1.9.23 binary outside the repo as an oracle and committing its outputs with provenance. Mercury's `-t` transmit test, FIFO audio and documented tools can generate frames. From recordings a person can recover waveform constants (sync sequences, tone mapping, UW and pilot patterns, an interleaver, an LDPC generator matrix). Every recovered table is still an approval item in section 7. Decision 2's ban on analysing signals names proprietary modems, and Mercury is not one. A recorded owner decision is a prerequisite for starting Route B.

## 9. Feature requests for the owner to file with Mercury

Sources for the requests: Mercury v1.9.23 `README.md`, `docs/CAROUSEL-ARQ.md` and `docs/RELEASE-1.9.18.md` through `docs/RELEASE-1.9.23.md`; codec2 documentation listed in section 3.4. These are proposed features, not claims that Mercury already implements them.

These are text for the owner. None has been filed. The owner files these requests. Titles follow the repo's outcome style.

### FR-1. A second implementer should be able to tell which air-protocol version a station speaks, and what stays compatible

Body: Mercury 1.9.18, 1.9.19 and 1.9.20 each broke the air protocol, and 1.9.21 and 1.9.23 each added to it. The release notes say so, but the wire has no single named version, and a mismatched station is dropped without a message. coppa is an open-source HF modem project studying interoperability from published documents.

For an example, coppa publishes its own air-protocol version marker and compatibility promise in `docs/SPEC.md` section 0.3.

Please consider: (a) naming the current generation in one place, for example "air protocol 2, minor N", and stating it on the wire in the connect frame; (b) a short compatibility policy, for example "within generation 2 only additive changes, unknown minor bits ignored, unknown packet types dropped"; (c) saying which of CAROUSEL-ARQ.md and CAROUSEL-SPEC.md sections are stale for the turn rules, since `CAROUSEL-TURNS.md` says the handover was removed.

```gherkin
Scenario: A second implementer targets a named generation
  Given Mercury publishes the air-protocol generation and its compatibility rules
  When a release changes only the minor number
  Then an implementation of the generation still connects
  And a release that breaks the generation says so in the same place
```

### FR-2. A second implementer should be able to check frames against Mercury without running two radios

Body: The request is for a small set of conformance vectors: for DATAC16, MFSK and at least one payload rung, an input byte string, the session or plain-CRC seed used, and the expected audio (WAV at 8 kHz) and decoded bytes. Include a CALL2 and an ACCEPT, one DATA frame, one POLL, an ACK pattern, and the block check example. Please state each vector's licence, so it can be committed with provenance.

```gherkin
Scenario: A vector set proves a frame is accepted
  Given a published vector for a CALL2 on DATAC16
  When an independent implementation produces the same audio from the same input
  Then Mercury's decoder reports the same bytes
```

### FR-3. A second implementer should be able to run Mercury offline, with no radio and no sound card

Body: Mercury has FIFO and socket audio backends, a receive test (`-r`) and a transmit test (`-t`). It is not documented how to feed a recorded WAV through the receiver and write a decoded frame to a file, or how to turn a byte string into a WAV with a chosen mode, seed and nonce. A documented offline encode and decode mode, with fixed random choices (session id, nonce) settable from the command line, would let outside projects run Mercury as a black-box oracle in CI.

```gherkin
Scenario: Offline loopback
  Given a recording of a Mercury transmission
  When it is fed to the offline decoder with the session nonce supplied
  Then the decoder writes the frame bytes and the SNR to a file and exits
```

### FR-4. A second implementer should be able to take the constant tables under a licence that fits

Body: Interoperability needs LDPC matrices, tone lists, unique words, interleaver parameters and the callsign coder model. Several are Mercury code (GPL-3.0) and several are codec2 (LGPL-2.1). Please say which tables are Mercury's, give each generator's seed and algorithm where a table is generated, and say whether you permit the values to be restated in a document under a permissive licence (the numbers themselves, not code). Please also state whether a clean-room specification written from your documents is welcome.

```gherkin
Scenario: Tables with a stated origin
  Given a published list of constant tables with source and licence
  When an implementer wants to reproduce one
  Then the list says whether the values may be restated in a permissively licensed spec
```

## 10. Sources

Mercury, tag v1.9.23 (`09f258f84a0b69cb4d8556530a25822617ea17dd`), GPL-3.0 project, paths in section 1 under `https://github.com/Rhizomatica/mercury/blob/v1.9.23/`. GitHub release pages `https://github.com/Rhizomatica/mercury/releases/tag/v1.9.18` to `v1.9.23`. codec2 `https://github.com/drowe67/codec2` (`README_data.md`, `README_ofdm.md`, `doc/modem_codec_frame_design.ods`), LGPL-2.1, `main` as fetched on 2026-11-10.

coppa: `CLAUDE.md`, `docs/DECISIONS/2026-11-09-clean-room-and-licensing-policy.md`, `docs/DECISIONS/2026-11-02-public-rust-api-semver-gate.md`, `docs/SPEC.md`, `docs/LIMITATIONS.md`, `docs/analysis/2026-07-03-world-class-gap-analysis.md`, `docs/reviews/2026-09-05-competitive-landscape.md`, and the Rust files named in section 8.1.

ETSI: EN 302 307-1 V1.4.1 (2014-11), clause 5.3 Table 5b and clause 5.3.2, linked in section 3.4. Only the code dimensions were checked; no standard table values are reproduced.

