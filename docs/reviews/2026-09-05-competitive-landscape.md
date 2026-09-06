# E — Competitive landscape and end-user jobs-to-be-done for coppa

Reviewer dimension: what hams actually run today, what they complain about, and where coppa can win.
Date: 2026-09-05. Repo read at `/Users/thagale/Code/coppa` (HEAD 2026-09-03, 231 commits, **no tags, no release
workflow, no binaries**). External research: ~45 primary/secondary pages, all listed in §6. Evidence labels used
throughout: **[measured]** = a number from a test harness or simulator; **[vendor]** = a claim by the product's own
author; **[forum]** = community sentiment; **[repo]** = read from coppa's checked-in source/docs.

## Summary

The HF data-modem market in 2026 is a three-way split: PACTOR-4 (SCS hardware, ~USD 1,000+, still the reference for
never-drop robustness and the SailMail/maritime segment), VARA HF (EA5HVK, closed-source, Windows-only, USD 69 per
callsign, free version throttled to ~175 bps, "quickly becoming the go-to mode in Winlink" and the engine under
VarAC's self-reported 90,000 users), and a long tail of open modems whose common story is *good intentions, weak
performance, and thin maintenance*: ARDOP is a 2016-era design that the Winlink team's own 2020 IONOS study found
"slow and less reliable" with "frequent connection drops"; ardopcf, the best open Linux/RPi implementation, has been
**discontinued by its author**; FreeDATA "development has slowed" per its own README; and only Mercury (Rhizomatica,
ARDC-funded, GPL-3, C, released 2026-05) is a live, funded, VARA-TCP-compatible open alternative — but it has *no
published head-to-head numbers* and no GUI. The FCC removed the HF symbol-rate cap on 2024-01-08 and replaced it with
a 2.8 kHz bandwidth cap; coppa's HF profiles (350–2800 Hz occupied, ≤2850 Hz TX filter) are inside it, but the
`vhf_wide` profile (350–5900 Hz) must never be used below 29.7 MHz in the US (it is legal in Canada's 6 kHz HF
allowance). coppa's honest position today: an unusually well-instrumented, well-specified Rust PHY/protocol stack
with a VARA-style TCP interface, 10 speed levels, NR-BG2 LDPC + IR-HARQ, AFSK/KISS, C FFI and WebSocket API — but
**zero over-the-air evidence, no release artifacts, no GUI, no Pat/Winlink integration path that has ever been
exercised, a README that still describes a BPSK-only prototype, and simulator numbers (BPSK-1/4 FER≤10% at 6 dB
AWGN, 3 kHz-referenced) that currently trail Mercury/FreeDV's published DATAC3 (0 dB MPP) and VARA's IONOS curves by
a wide margin.** The biggest competitive openings are (1) the Linux/RPi Winlink gateway and Pat-user segment that
VARA-under-Wine serves badly and ardopcf just abandoned, (2) the "first open modem with credible public benchmarks"
position that Mercury has left vacant, and (3) the Rust/C-FFI DSP-library niche that no competitor occupies at all.
None of those is winnable without first shipping binaries, an OTA test, and a Pat transport.

---

## 1. Landscape: what each alternative actually is (with evidence)

### 1.1 VARA HF / VARA FM (EA5HVK, José Alberto Nieto Ros)

- **Licence/price**: closed-source Windows binary; free version limited to the lowest speed levels ("~175–180 bps")
  [vendor/forum: sigidwiki, wa8lmf.net]; paid licence USD 69 per callsign unlocks all levels for HF *and* FM, usable
  on any number of machines under that callsign [forum: VARA-MODEM groups.io, EMA ARRL group buy]. New central site
  varamodem.com launched 2026 (Zero Retries 0250); fetch failed (TLS) so pricing not re-verified there.
- **Versions** (rosmodem.wordpress.com, read 2026-09-05): VARA HF 4.9.0, VARA FM 4.4.0, VARA SAT 4.4.5, VARA Chat
  1.4.2, VARA Terminal 1.2.2.
- **Waveform** [vendor, sigidwiki + VARA spec rev 2.0.0]: OFDM, 17 speed levels, 500/2300/2750 Hz modes; symbol
  rates 23/42/94 baud; FSK at levels 1–4, 4PSK 5–9, mixed 4/8PSK/16QAM 10–14, 16/32QAM 15–17; 2–59 carriers; net
  rates **18 bps (L1) → 8,489 bps (L17)**; the author's own tagline is "uncompressed User Data Rate to 5629 bps at
  S/N 14.5 dB @ 4 kHz" and "37.5 bps symbol rate with 52 carriers". VARA FM: "25,210 bps max" (Wide mode).
- **Measured** [Winlink IONOS study, N5TW, Nov 2020, Teensy Watterson simulator, bytes/min net after ARQ]: VARA 2300
  ≈ 46,000 B/min at 30 dB WGN (beats PACTOR-4's 40,000), ≈ 23,500 at 20 dB, ≈ 11,000 at 15 dB, ≈ 5,000 at 10 dB,
  ≈ 2,000 at 5 dB; on MPP (Poor, 1 Hz/2 ms) ≈ 18,000 at 30 dB falling to ≈ 1,000 at 0 dB. VARA 500: ≈ 10,400 B/min
  at 30 dB WGN, ≈ 5,800 at 15 dB, ≈ 3,000 at 10 dB; PACTOR-2 beats it below ~10–14 dB. Headline: "SCS and VARA proved
  extremely reliable over all the test conditions NEVER dropping a connection."
- **Host apps that depend on it**: Winlink Express, RMS Trimode (gateway), Pat (via `pat-vara`, v1.2.0 June 2025),
  VarAC, VARA Chat/Terminal, BPQ32, JNOS, RadioMail (iOS, via a VARA running on a *separate* computer), VARAtrack,
  TPRFN. K6AQ's Winlink-gateway talk: "VARA is quickly becoming the 'go to' mode in Winlink."
- **Complaints** [forum]: closed source; Windows-only so Linux/Mac need Wine (Winlink's own instructions say the
  RPi/ARM route "is not realistic or stable enough — use an Intel SBC"); on RPi, Box86+Wine works only with specific
  box86 commits (regression after 2021-12-10; crash on Dynarec v0.3.1), the CPU gauge is broken under Wine, CM108
  HID PTT not recognised, "Wine can freeze the OS under heavy load"; **RPi4↔RPi4 VARA links failed outright until
  real-time thread priority hacks were applied because Wine/Box86 latency exceeded VARA's link-establishment
  timeouts** (eindhoven.space, 2022); Winelink's maintainer "doesn't have much time to maintain" it; Mac M1 users
  stuck ("wine does not work and ARM windows64 has issues", pat-users). Licensing per callsign and the ~175 bps
  free cap are recurring irritants but most users pay. CPU-load complaints exist but are mostly about old laptops.

### 1.2 ARDOP / ardopc / ardopcf (the Winlink "open" modem)

- **Status** [primary: winlink.org ARDOP pages, ardop.groups.io, pflarue/ardop]: ARDOP 1 (2016, Rick Muething KN6KB)
  is the only variant Winlink supports; ARDOP2 "generally performs better but is not supported by Winlink";
  ARDOP1OFDM / ARDOPOFDM (John Wiseman G8BPQ) add OFDM modes "significantly faster and more robust" but "not
  supported by Winlink software" and described by Wiseman as "a bit of a rush job after ARDOP2 was abandoned";
  ARDOP3 abandoned, ARDOP3K "still not fully functional". Bandwidths 200/500/1000/2000 Hz.
- **ardopcf** (Peter LaRue AI7YN, MIT): the polished Linux/Windows/ARM fork with a built-in browser web GUI, single
  static binaries for Linux x86-64/ARM32/ARM64 and Windows, tested on RPi Zero/Zero 2/4B, Hamlib/rigctld PTT,
  documented "Host Interface Commands" TCP protocol used by Pat, WoAD, RadioMail. **The README now says: "I have
  decided to discontinue development of ardopcf and will not be accepting any additional pull requests."** He lists
  successors: ARDOP_WIN (Muething), g8bpq/ardop, DL2MAN's browser-only "ARDOP Winlink" (MIT, in-browser JS modem +
  B2F client, registered with Winlink as `ARDOPCHROME`), and "ardopb by Ryan Barber — an AI driven
  re-implementation". His stated motivation: enable RPi Winlink where VARA can't run; he concedes "Ardop does not
  provide the same performance as the commercial VARA HF software".
- **Measured** [IONOS 2020]: ARDOP 2000 ≈ 10,000 B/min at 30 dB WGN but ≈ 2,500 at 15 dB and ≈ 1,300 at 10 dB;
  ARDOP 500 ≈ 2,900 B/min WGN, collapsing to ~500–900 on MPG/MPP at 15 dB; "often required multiple runs to get
  tests to complete at low SNRs" — i.e. connection drops.
- **Why people still prefer VARA**: 3–5× throughput at the same SNR, no drops, and Winlink Express + gateways treat
  it as first-class. **Why ARDOP survives**: it is the only *Winlink-accepted* protocol with a native Linux/ARM/iOS
  implementation (RadioMail has ARDOP built in).

### 1.3 FreeDATA (Simon Lang DJ2LS)

- GPL-3, Python server + Vue/Electron GUI + REST API; codec2/FreeDV OFDM `datac*` modes (500–1700 Hz; see §1.4
  table); messaging, file transfer, ARQ, beacons, 0.18.x added broadcast; 209 GitHub stars / 33 forks; a PyPI
  package that "is not yet working"; README: **"Development has slowed due to maintainer capacity constraints"**,
  recommends staying on 0.17.8 if 0.18.x misbehaves; Zero Retries: "there doesn't seem to be much development
  activity on FreeDATA of late." No Winlink/Pat integration; its own message system only.
- **Lesson**: the architecture (headless server + REST + separate GUI, statistics.freedata.app spotting map) is
  right; a single volunteer maintainer plus a Python install story is what stalled it.

### 1.4 Mercury (Rhizomatica / Rafael Diniz PU2UIT) — coppa's closest competitor

- Released 2026-05-07 as "Mercury v2" (C rewrite, 679 commits on the `mercuryv2` branch; v1 was C++), GPL-3.0,
  ARDC-funded (2021/2023/2025 grants), part of HERMES. Debian 13 arm64/amd64 `apt` packages, Windows installer,
  macOS universal `.dmg`, RPi supported; audio backends alsa/pulse/oss/coreaudio/aaudio/dsound/wasapi + shm/null/
  fifo for testing.
- **Waveform**: FreeDV/codec2 OFDM data modes plus Rhizomatica's own DATAC15/16 (200 Hz, rate-1/3, −7 dB AWGN /
  −4 dB MPP), DATAC17 (2100 Hz, 1410 bps, +10 dB MPP) and QAM16C2 (2100 Hz, 3100 bps, +15.7 dB MPP); HARQ with
  Chase combining ("0/130 → 61/130 frames at −5.8 dB"), OLLA outer-loop link adaptation, per-direction mode
  selection, split control/data channel. Published `docs/MODES.md` table with per-mode SNR thresholds
  **[measured, self-published]**; stop-and-wait ARQ goodput "DATAC1 ≈ 50 B/s, DATAC17 ≈ 98 B/s, QAM16C2 ≈ 142 B/s"
  (i.e. ~1.1 kbps peak — well under VARA's 8.5 kbps ceiling). 32-tone MFSK mode still being ported.
- **Host interface**: VARA-style two-socket TCP TNC: MYCALL (+4 aux), LISTEN ON/OFF/CQ, CONNECT, DISCONNECT, ABORT,
  PUBLIC, BW500/2300/2750, COMPRESSION (no-op), CHAT, P2P (no-op), CALLINT, RETRIES, CQFRAME, TUNE, BUFFER, SN,
  BITRATE, VERSION, IGNOREKISSDCD; async PTT ON/OFF. **No CWID**. Plus a KISS-over-TCP broadcast port verified
  with Reticulum (`docs/RETICULUM.md`). ARRL: "compatible with VarAC … function like any other modem once
  configured"; "currently does not have a user interface".
- **Claims vs VARA** [vendor]: "nearly at parity in optimal SNR conditions and outperforms the alternative in poor
  SNR conditions"; Diniz: "better performance on high-SNR links". **No head-to-head numbers published anywhere I
  could find.** No Winlink CMS / RMS Trimode acceptance; no Pat transport documented.
- **Take**: Mercury is 12–18 months ahead of coppa on packaging, funding, and community narrative ("the VARA
  replacement" per Amateur Radio Newsline, ICQ, HamWeekly, ARRL), but behind on waveform ambition (peak ~3 kbps
  raw / ~1.1 kbps goodput vs coppa's 64-QAM ladder) and on published rigor (no CIs, no simulator matrix).

### 1.5 PACTOR (SCS)

- Hardware modem; DR-7400 discontinued in favour of DR-9400; used DR-7400 ≈ USD 1,100 (2023, cruisersforum); new
  units historically USD 1,500–2,000 (retail page fetch was 403; treat as approximate). PACTOR-4 dominated every
  IONOS multipath case ("PACTOR 4 dominates over the realistic SNR range"), never dropped a link. SailMail (USD 275/yr,
  no ham licence needed, marine bands) **requires PACTOR**; all SailMail stations support P4. Winlink HF gateways
  running RMS Trimode commonly offer PACTOR + VARA + ARDOP on the same port.

### 1.6 The rest of the field (brief, with what was actually read)

- **Winlink ecosystem**: RMS Trimode (Windows gateway; ARDOP/VARA/PACTOR), Winlink Express (Windows; session-type
  dropdown Telnet/Packet/Pactor/ARDOP/VARA HF/VARA FM/Iridium), Pat (Go, Linux/mac/Win; transports ardop, vara,
  pactor, ax25, telnet; VARA "runs great on Wine"), RadioMail (iOS; built-in packet + ARDOP, VARA via remote
  computer, auto station directory by proximity, 100+ forms, GPS prefill, background receive), WoAD (Android).
  Winlink's "Open Letter" position: dynamic compression is not encryption; protocols must be publicly documented so
  third parties can monitor; Winlink Message Viewer exists for self-policing. Gateways connect *outbound* to CMS so
  no port-forwarding is needed. K6AQ: "Software also runs fine on Raspberry Pi 4Bs running Windows 10!" — i.e. the
  community's answer to RPi gateways today is *Windows on ARM*, not Linux.
- **VarAC** (Irad Deutsch 4Z1AC): free, Windows, V15.0.18; "90,000 amateur radio operators in over 100 countries"
  [vendor]; 18 releases in its first year. Killer features: calling frequencies + **slot-based QSY** (CQ on the
  calling frequency, auto-QSY the pair to a free slot via CAT/Omnirig), beacons, ping-before-connect, SNR shown per
  station, VMail store-and-forward with offline compose, file/image transfer, PSKReporter self-spotting + "who
  spotted my beacon" button, spell-check, dark mode, auto mailbox. It is "a nightmare at Linux" (VarAC forum).
- **Direwolf** (WB2OSZ): the default AFSK TNC because it is free, decodes more frames than hardware TNCs ("over 1000
  error-free frames from Track 2 of the WA8LMF TNC Test CD"), is in every distro (`apt`, `brew`), speaks KISS over
  TCP/serial/Bluetooth *and* AGWPE, supports 300/1200/2400/4800/9600, FX.25 and IL2P, EAS/AIS, up to 3 sound cards
  and 6 radios, and PTT via GPIO/CM108/serial/hamlib. TheModernHam: "there is almost no reason to get [a hardware
  TNC] today"; the residual objection is gateway-uptime reliability of a PC.
- **UZ7HO Soundmodem**: Windows AFSK/FX.25 modem used in the IONOS VHF test (site fetch 404'd); VARA FM "crushed"
  AX.25/FX.25 in that test (≈140,000 B/min vs ≈5,000).
- **JS8Call** (now JS8Call-Improved 3.0.3, July 2026): −24 dB weak-signal keyboard messaging with relay,
  store-and-forward, heartbeat/grid, APRS-iGate; Windows/Linux (x86-64 + ARM64)/macOS (Intel + Apple Silicon).
  Not a bulk-data competitor; it *is* the competitor for "beacon/ALE-class low-rate mode".
- **Reticulum**: consumes any modem that exposes **KISS over serial or TCP** (`kiss_framing`); no HF modem is named
  in its manual; Mercury verified itself as a Reticulum interface. This is the cheapest ecosystem entry for coppa.
- **M17**: open 4FSK 9600 sym/s digital voice/data for VHF/UHF, Codec2, CS7000-M17 handheld (July 2024), 130+
  reflectors; adjacent, not a competitor.
- **Meshtastic / NPR / QRadioLink / Winlink Express modem-UX internals**: *not researched from primary sources in
  this pass* (rate-limit budget); treated as adjacent only.

### 1.7 Regulatory context

- **USA**: FCC 23-93 (adopted Nov 2023, effective **2024-01-08**) removed the HF symbol-rate limit and set
  §97.307(f)(3) "authorized bandwidth is 2.8 kHz" for RTTY/data 160–10 m (60 m also 2.8 kHz per (f)(14); 2200/630 m
  keep 300 baud). VHF/UHF (f)(5)/(6) are bandwidth-limited (20/100 kHz), no symbol-rate cap on 6 m/2 m/70 cm was
  found in the text read. §97.309(a)(4): any *publicly documented* technique (CLOVER, G-TOR, PACTOR cited) is a
  "specified digital code"; §97.309(b): unspecified codes must not be used "for the purpose of obscuring the
  meaning". §97.119: ID at the end and every 10 minutes; for data emissions ID may be sent "by a RTTY emission using
  a specified digital code" — i.e. in-band ID in a publicly documented mode is permitted, CW ID is the belt-and-braces
  convention (VARA has CWID; Mercury does not).
- **Canada**: RBR-4 allows 6 kHz below 28 MHz (2.8 kHz on 5 MHz). **IARU Region 1** band plan: segments of max
  200/500/2700 Hz (6000 Hz on 10 m); unattended EmComm stations limited to 2700 Hz; digimodes are *preferred* in
  labelled segments but any mode may use any segment within its bandwidth cap.
- **Alignment check for coppa** [repo: `docs/SPEC.md` §1.2, §11]: `hf_standard`/`hf_robust` occupy 350–2700 Hz,
  `hf_wide` 350–2800 Hz, `hf_narrow` 350–800 Hz, TX bandpass 250–2850 Hz — inside the FCC 2.8 kHz audio-bandwidth
  cap and the IARU 2700 Hz segment cap (**`hf_wide` at 2800 Hz would exceed IARU R1's 2700 Hz digimode segments
  and should be flagged as "US/Canada only" in docs**). `vhf_wide` (350–5900 Hz) is illegal on US HF and must be
  gated off below 29.7 MHz by the daemon, not just by convention; `select_ofdm_profile` currently routes *every*
  speed level ≥ 5 to `vhf_wide()` (CLAUDE.md, Bug A note) — **that is a regulatory-facing defect if any user runs
  levels 5–10 on HF**. coppa's Huffman+LZ4 compression is "publicly documented" by the SPEC, which is exactly what
  §97.309(a)(4) and Winlink's Open Letter ask for; a Message-Viewer-style decoder tool would close the loop.
  Station-ID is present as a "forced-level frame (station ID / beacon)" in `coppa-engine` [repo] but there is no
  CWID and no documented 10-minute timer in the daemon config beyond a `[station]`-style stanza — needs verifying
  and documenting.

---

## 2. Jobs-to-be-done: who the user is, what they run, what would make them switch

| Job | Who | Runs today | Switch trigger | What coppa must offer |
|---|---|---|---|---|
| Winlink email over HF — **gateway operator** | ARES/RACES sysops, club stations; runs 24/7 | RMS Trimode on a dedicated Windows PC (K6AQ: 4 GB/64 GB, USD 100–150 mini-PC, or RPi4 + Windows-on-ARM); PACTOR + VARA + ARDOP | A gateway that is Linux-native, headless, systemd-managed, stable for months, *and* that Winlink CMS accepts | A Trimode-equivalent (B2F server, CMS link) **or** a modem that plugs into an existing Linux gateway stack (BPQ32/LinBPQ speaks VARA-TCP already). Winlink acceptance is the moat: VARA/ARDOP got in because the WDT integrated them into Trimode/Express after BPQ32 beta tests and "many positive real-world performance reports". The realistic path is **LinBPQ/BPQ32 (accepts any VARA-TCP modem) → field reports → WDT interest**, not asking the WDT first. |
| Winlink email over HF — **field user** | EmComm volunteers, portable/POTA, RVers | Winlink Express + VARA on a Windows laptop (dominant); Pat + ardopcf or Pat + VARA/Wine on Linux/RPi; RadioMail on iPhone | An open modem their *gateway* also runs (chicken-and-egg), an install that is one command, and no Wine | Pat transport (`pat-coppa` or make coppa speak enough VARA-TCP that `pat-vara` works unmodified — cheaper), single static binaries, `apt`/`brew`, a "which gateways accept coppa" list. |
| EmComm exercises (ARES/RACES nets, SET) | Same population, but *net control* cares about forms, ICS-213, reliability, not bps | Winlink Express forms; VARA FM on VHF for local; fldigi/flmsg NBEMS | Nothing until gateways/peers run it | VARA-FM-class VHF mode (coppa has `vhf_*` profiles; VARA FM Wide is 25 kbps on 9600-capable radios) and P2P session mode with Winlink Express-style "P2P" semantics. |
| Keyboard-to-keyboard chat | The VarAC crowd (largest live-HF-data community) | VarAC + VARA HF, Windows; calling freqs + slots | An open modem VarAC can drive (Mercury already claims this) **and** a Linux-native chat client | Full VARA-TCP command surface so VarAC/VARA Chat "just work"; then a coppa chat client on Linux/mac (VarAC is "a nightmare" on Linux) with slots, beacons, ping, SNR display, PSKReporter spotting. |
| File/image transfer | VarAC users, FreeDATA users, HERMES community networks | VarAC file transfer; FreeDATA; Mercury/hermes-broadcast | Faster than FreeDATA, easier than Winlink | Native file-transfer app over coppa ARQ with resume, image downscaling, checksums; broadcast (one-to-many, no ARQ) mode like Mercury's hermes-broadcast. |
| Beacons/spotting | Everyone doing propagation checks | JS8Call heartbeats, VarAC beacons → PSKReporter, FreeDATA statistics site | A map that shows coppa activity | Forced-level beacon frame exists [repo]; add a PSKReporter/own-map reporter and a decoder that spots *other* stations' beacons. |
| Maritime / SailMail | Cruising sailors (a paying segment: USD 275/yr) | PACTOR-4 hardware (required by SailMail), Winlink for licensed hams | Not switchable: SailMail mandates PACTOR; Winlink-at-sea users need a *gateway* that runs coppa | Long-term only; requires Winlink acceptance first. |
| Off-grid / preppers / community networks | Reticulum, Meshtastic, HERMES/Rhizomatica deployments | Mercury (HERMES), LoRa/RNode, Meshtastic | KISS-over-TCP so Reticulum can use it; broadcast mode | coppa already has a KISS TCP server for AFSK [repo: `coppa-daemon/src/tnc.rs`, port 8001]; expose the OFDM HF modem behind KISS too. |
| Developer / researcher | DSP students, SDR hackers, modem authors, MIL-STD/STANAG people | GNU Radio, codec2 (C), MATLAB; no Rust option | A clean, tested, documented library with channel models and a bench harness | This is coppa's strongest existing asset (13 crates, SPEC.md, Watterson models, `coppa-bench`, C FFI, WebSocket JSON). Needs crates.io publication, docs.rs, examples, and Python bindings. |
| Education | Clubs, universities | fldigi, WSJT-X, codec2 | Visualisation | Waterfall/constellation/LLR visualiser in a web GUI; the WebSocket API is the right substrate. |

**The chicken-and-egg problem, concretely**: VARA broke in because (a) EA5HVK shipped a TCP TNC interface that BPQ32
could drive the same day, (b) BPQ32 gateway operators ran it and posted results, (c) the WDT added it to Trimode and
Express as a beta with auto-update, (d) the IONOS study then gave everyone a number to point at. ARDOP got in because
the WDT *wrote* it. Mercury is attempting (a)+(b) now. coppa has (a) partially (VARA-style TCP) but has never been
driven by BPQ32, Pat, VarAC or Winlink Express — that is the single cheapest experiment to run.

---

## 3. Feature/capability comparison table

Legend: coppa cells cite `[repo:file]`; "?" = no evidence found; "n/a" = not applicable.

| Capability | **coppa** | VARA HF | ARDOP (ardopcf) | FreeDATA | Mercury v2 | PACTOR-4 |
|---|---|---|---|---|---|---|
| Open source | Yes, MIT/Apache-2.0 [repo] | No (closed binary) | Yes, MIT (protocol spec public) | Yes, GPL-3 | Yes, GPL-3 | No (hardware, proprietary) |
| Licence / price | Free | USD 69/callsign; free tier ~175 bps | Free | Free | Free | ~USD 1,100 used (2023); new higher |
| Platforms | Builds Linux/macOS/Windows in CI [repo: ci.yml]; **no binaries, no tags** | Windows only; Wine/Box86 on Linux/RPi with caveats | Linux x86-64/ARM32/ARM64, Windows; RPi Zero–4B tested | Win/Linux/mac (Python + Electron) | Linux (Debian apt amd64/arm64), Windows, macOS dmg, RPi | Hardware; drivers for Win; works with Pat/Trimode |
| Bandwidth modes | 500 Hz (`hf_narrow`), 2400 Hz (`hf_standard`/`robust`), 2450 Hz (`hf_wide`), 5.5 kHz VHF [repo: SPEC §1.1] | 500 / 2300 / 2750 Hz | 200 / 500 / 1000 / 2000 Hz | 250–1700 Hz codec2 modes; 500/2438 Hz OFDM WIP | 200 / 250 / 500 / 1700 / 2100 Hz | 500 Hz (P1–2), 2400 Hz (P3/4) |
| Max throughput | **Sim only**: peak goodput 8,450 bps (64QAM 5/6, `vhf_wide`, AWGN, this review's dim-F re-sweep) [repo: `results/review-2026-09-05/`]; **no OTA number**, and this figure requires the VHF-bandwidth profile -- the SSB-realistic HF ladder tops out far lower (see dim-F) | 8,489 bps L17 [vendor]; ≈46 kB/min at 30 dB WGN [IONOS measured] | ≈10 kB/min (≈1.3 kbps) at 30 dB WGN [IONOS] | ≈980 bps (datac1) raw | 3,100 bps raw (QAM16C2); ≈142 B/s goodput under stop-and-wait ARQ [self-published] | ≈40 kB/min (≈5.5 kbps) [IONOS]; 10.5 kbps spec |
| Min-SNR sensitivity | BPSK 1/4 (level 1) FER≤10% at **6 dB AWGN, 12 dB Watterson-Good/Moderate, 30 dB Poor** (this review's dim-F re-sweep, 3 kHz-ref) [repo: `results/review-2026-09-05/`]; MIL-STD ladder passes 0/27 | ≈2,000 B/min still flowing at 5 dB WGN; L1 at 18 bps designed for negative SNR [IONOS/vendor] | ARDOP 500 works to ~5 dB WGN but drops on MPP [IONOS] | datac4 90/100 at −4 dB MPP, datac3 74/100 at 0 dB MPP [codec2 measured] | DATAC16 −9.3 dB AWGN / −4 dB MPP; DATAC17 +10 dB MPP [self-published] | P4 ≈2,000 B/min at 0 dB WGN; never drops [IONOS] |
| ARQ | Selective-repeat + IR-HARQ (NR BG2), multi-codeword frames; session bench drop-free on only 2/5 Good, 0/5 Mod/Poor [repo: dim-F report, COP-3 re-run] | Yes, adaptive; never dropped in IONOS | Yes; frequent drops on MPP | Yes | HARQ + Chase combining, OLLA | Yes, memory-ARQ |
| Compression | Huffman + LZ4 [repo] | Yes (proprietary) | Host-side (Winlink LZHUF) | Yes | COMPRESSION cmd is a no-op | Yes (PMC) |
| Host API | VARA-style TCP 8300/8301: **implemented** LISTEN, PTT ON/OFF, BUSY ON/OFF (twitchy, no hysteresis), BUFFER (emitted, but counts frames not bytes), TUNE (extra, verified real through the transmit path); **partial** CONNECT/DISCONNECT (parsed and transmitted, but no `OK`, `CONNECTED`/`DISCONNECTED` generated then dropped before reaching the socket); **no-op** (parsed, ignored) MYCALL, ABORT, COMPRESSION, BW500/2300/2750; **missing** CWID, PUBLIC, CQFRAME, SN, BITRATE, CHAT, WINLINK/P2P SESSION, `OK`/`WRONG`, `IAMALIVE` [repo: coppa-host; dim-C probe]. Separately: WebSocket JSON; KISS-over-TCP for AFSK (a distinct protocol/port, not a VARA command); C FFI | VARA TCP (de-facto standard) | ARDOP host protocol TCP 8515 | REST + WebSocket | VARA-style TCP + KISS-over-TCP | Serial/USB AT-style |
| GUI | None | Windows GUI with waterfall/S-N/CPU gauges | Built-in browser web GUI | Electron GUI | None ("requires familiarity with the terminal") | Front-panel + host apps |
| Winlink support | None, never tested | First-class (Express, Trimode) | First-class | None | None (VARA-TCP may let BPQ32/Pat work — unverified) | First-class |
| Pat support | None; `pat-vara` *might* work if the VARA subset is sufficient — untested | Yes (`pat-vara`) | Yes (built-in) | No | Untested | Yes |
| Active community | 1 org, 231 commits, 0 releases, no forum | Large; groups.io VARA-MODEM; 90k VarAC users [vendor] | ardop.groups.io; **author discontinued** | 209 stars; Discord; slowed | ARDC-funded; mailing list; press coverage May 2026 | SCS + marine dealers |
| Release cadence | No releases | HF 4.9.0; steady | v1.0.4.1.3 (Nov 2024) then stopped | 0.18.x, slowed | v2 May 2026, active | Firmware updates |
| RPi support | Should compile (aarch64 not in CI) [repo] | Wine/Box86 hacks; latency breaks links | Yes, tested Zero–4B | Yes (heavy) | Yes (apt arm64) | Via USB |
| Ease of install | `cargo build` from source only | Download .exe + licence key | Download one static binary | Script installer / PyPI (broken) | `apt install` / .exe / .dmg | Buy hardware |
| Published benchmarks | Extensive *simulator* FER/goodput with CIs, Watterson presets, MIL-STD ladder [repo: BENCHMARKS.md] — the most rigorous in the field, but all negative-to-mixed and none OTA | IONOS study (third-party) | IONOS study | codec2 README_data | `docs/MODES.md` self-published thresholds | IONOS study |

**Evidence gaps in coppa's column that must be closed before any external comparison is credible:**
1. No over-the-air result of any kind (two radios, or even two sound cards through a Teensy IONOS simulator, which
   costs < USD 200 and is what the Winlink team used).
2. Two contradictory AWGN tables in BENCHMARKS.md (0 dB vs 12 dB FER≤10% for BPSK) because the SNR reference
   bandwidth changed mid-project; only the 3 kHz-referenced numbers are comparable to VARA/Mercury/MIL-STD.
3. No IONOS-comparable bytes/minute-under-ARQ sweep at fixed SNR points across WGN/MPG/MPP. A simulated session-bench bytes/min number does exist (dimension-F report; BENCHMARKS.md), but it is not run at the IONOS study's fixed-SNR grid, so it cannot be plotted directly against the VARA/ARDOP/PACTOR curves in this table.
4. README status table is stale (says OFDM "Partial", QPSK+ "not wired") — a
   competitor reading it would conclude coppa is a BPSK toy.

---

## 4. Where coppa can win — wedge strategies

### W1. "The open modem for Linux/RPi Winlink gateways and Pat users" (highest leverage, medium effort)
- **Why open**: VARA on RPi is Wine+Box86 with link-timing failures; ardopcf is discontinued; ARDOP itself is slow;
  Mercury has apt packages but no Winlink story and no CWID. Gateway sysops already run LinBPQ, which speaks
  VARA-TCP.
- **Required**: (1) finish the VARA-TCP surface to the level Mercury documents (PUBLIC, CQFRAME, SN, BITRATE,
  BUFFER, CWID, LISTEN CQ, TUNE) so `pat-vara`, LinBPQ and VarAC drive coppa unmodified; (2) static binaries for
  linux-x86-64/arm64/armv7 + Windows + macOS, `apt`/`brew`/`cargo binstall`; (3) systemd unit + `coppad` health
  endpoint; (4) a documented two-station OTA test and an IONOS-simulator run; (5) recruit 3–5 LinBPQ gateway
  operators to run coppa alongside VARA and post connect logs; (6) only then approach the WDT with data.
- **Honest risk**: coppa's simulated fading robustness (drops on Moderate/Poor sessions) is currently *worse* than
  ARDOP's reputation, and gateway operators will judge on drops, not peak bps. W1 is blocked on PHY robustness
  work owned by other reviewers; the packaging/interface work can proceed in parallel.

### W2. "The only open HF modem with credible public benchmarks" (positioning, small-to-medium effort)
- Mercury claims VARA parity with zero numbers; ARDOP's only numbers are third-party and unflattering; FreeDATA has
  none. coppa already has the harness, Watterson presets and committed, reproducible FER tables (though not currently regenerated or checked in CI -- `ci.yml` only compile-checks benches). Publishing a **standing scoreboard
  page** (bytes/min vs SNR for WGN/MPG/MPP in the exact IONOS format, with CIs, plus a Teensy-IONOS hardware run)
  would make coppa the reference everyone else is measured against — including when coppa loses. Requires: fix the
  SNR-convention ambiguity, add bytes/min-under-ARQ to `coppa-bench`, buy/build an IONOS simulator, publish.

### W3. "The Rust DSP/modem library" (unique, small effort, compounding)
- No competitor is a library. codec2 is C; Mercury is a C daemon; VARA is a binary. coppa's 13-crate split, C FFI,
  channel models and SPEC.md are already library-shaped. Required: publish crates to crates.io with docs.rs,
  semver, `no_std`-friendly `coppa-dsp`/`coppa-codec` where feasible, PyO3 bindings, 5 runnable examples
  (WAV encode/decode, channel sim, custom profile, FFI from C, WebSocket client). This wedge earns contributors,
  which is what FreeDATA and ardopcf lacked.

### W4. "The VARA-compatible open modem for VarAC/chat and Reticulum/HERMES-style community networks"
- Same TCP surface as W1, plus a KISS-over-TCP broadcast/datagram mode for Reticulum (Mercury has verified this) and
  a hermes-broadcast-style one-to-many file mode. Rhizomatica's HERMES deployments are GPL and Mercury-first, so
  coppa's pitch there is licence (MIT/Apache) and throughput ladder, not incumbency. Medium effort; depends on W1's
  interface work.

### W5. "VARA-FM-class VHF/UHF data" (later)
- VARA FM Wide (≈25 kbps, 9600-capable radios) has no open competitor at all; Direwolf tops out at 9600 GMSK.
  coppa's `vhf_wide` profile (104 carriers, 350–5900 Hz) is the seed. Requires a VHF-specific OTA program and
  FM-radio audio-path characterisation; not before W1–W3.

---

## 5. UX lessons from the winners (concrete, copyable)

| From | What they do | What coppa should copy |
|---|---|---|
| **VARA** | One .exe, one licence key field, one "Sound card / PTT / BW" dialog; TCP TNC so every host app integrates; visible S/N and speed-level gauge during a session; CWID option; BUSY detector. | Ship `coppad` as one binary with `coppad --setup` wizard writing `coppad.toml`; expose live SNR/level/BUFFER over both TCP and WebSocket; add CWID + configurable 10-min ID; expose BUSY state (spectrum sensing exists in `coppa-ml`). |
| **VarAC** | Calling frequency + slot auto-QSY; ping before connect; beacons + PSKReporter; SNR in the station list; VMail store-and-forward; dark mode; spell-check; 18 releases in year one. | A coppa chat client (web UI over the WebSocket API) with ping/beacon/slot semantics and PSKReporter self-spotting; release monthly with a changelog people can read. |
| **Direwolf** | Text config file with copious comments; in every package manager; KISS *and* AGW so nothing needs to change upstream; decodes better than hardware and says so with a reproducible test-CD number; multi-radio. | `apt`/`brew`/`winget` packages; AGWPE port alongside KISS for the AFSK TNC (Winlink Express/Outpost expect AGW); a reproducible "TNC Test CD" style number for coppa's AFSK decoder. |
| **ardopcf** | Static single-file binaries per arch; built-in browser GUI (no install, phone-usable); `--hostcommands`; RPi-tested matrix in the release notes; documented Host Interface Commands. | Same: web GUI served by `coppad`, per-arch static binaries, a tested-hardware table. |
| **FreeDATA** | Headless server + REST + separate GUI; statistics/spotting site; Discord. | Keep the daemon/GUI split; stand up an activity map fed by beacons. |
| **WSJT-X/JS8Call** | Waterfall with click-to-tune, decoded-station list with SNR/grid, everything on one screen; multi-arch builds including ARM64/Apple Silicon. | Waterfall + constellation + LLR histogram in the web GUI; ARM64 and Apple-Silicon binaries. |
| **Mercury** | `docs/MODES.md` per-mode SNR table, `docs/TNC.md` command table, `docs/RETICULUM.md`; apt repo; press kit ("VARA replacement") on day one. | Publish equivalent `docs/MODES.md` (from BENCHMARKS.md), `docs/TNC.md` (VARA command coverage matrix: supported / no-op / missing), and a one-page press-style project page. |
| **RadioMail** | Auto station directory by GPS proximity; forms prefilled; background receive with notifications. | Out of scope for a modem, but the host-API should expose everything (SNR, connect state, remote call) a RadioMail-class client needs. |

---

## 6. Hit list

Impact H/M/L = effect on end-user adoption; Effort S/M/L = < 1 week / 1–4 weeks / > 1 month.

| # | Item | Category | Impact | Effort | Evidence / source |
|---|---|---|---|---|---|
| 1 | Ship tagged releases with static binaries: linux x86-64/arm64/armv7, windows x86-64, macOS universal; `cargo binstall`, GitHub Releases | platform | H | M | No tags/release workflow in repo; ardopcf/Mercury both ship per-arch binaries (github.com/pflarue/ardop/releases; Mercury README) |
| 2 | `apt` repo (Debian/RPi OS) and Homebrew tap | platform | H | M | Direwolf is in apt/brew; Mercury ships Debian 13 apt for amd64/arm64 |
| 3 | Add aarch64-linux and RPi to CI (cross or QEMU), publish a tested-hardware table (Pi Zero 2/4/5) | platform | H | S | ardopcf release notes list "tested on Raspberry Pi Zero, Zero 2, 4B"; RPi is the Linux Winlink gateway target (eindhoven.space, K6AQ talk) |
| 4 | Complete the VARA TCP command surface to Mercury's documented set: PUBLIC, LISTEN CQ, CQFRAME, SN, BITRATE, BUFFER, TUNE, CHAT, P2P/CALLINT/RETRIES no-ops, CWID, IGNOREKISSDCD; publish `docs/TNC.md` coverage matrix | missing-feature | H | M | coppa-host only matches MYCALL/LISTEN/CONNECT/DISCONNECT/ABORT/BW*/COMPRESSION/PTT/VERSION/BUSY/KISS [repo grep]; Mercury docs/TNC.md |
| 5 | Verify `pat-vara` drives coppa unmodified; if not, write `pat-coppa` transport; document Pat config | ecosystem | H | S–M | getpat.io transports; n8jja/pat-vara v1.2.0 (June 2025) uses TCP 8300/8301 |
| 6 | Verify LinBPQ/BPQ32 can use coppa as a VARA-type port; recruit 3–5 gateway sysops for a shadow deployment | ecosystem | H | M | VARA got into Winlink via BPQ32 beta first (winlink.org "ARDOP and VARA now beta testing") |
| 7 | Verify VarAC and VARA Chat drive coppa on Windows; publish a VarAC setup page | ecosystem | H | S | Mercury claims VarAC compatibility (ARRL Mercury page); VarAC 90k users [vendor] |
| 8 | Two-station over-the-air test (even cross-room on 10 m/dummy load) with logged bytes/min, SNR, drops; publish WAVs | evidence-gap | H | M | Zero OTA evidence in repo; CLAUDE.md admits "still no live two-radio field test" |
| 9 | Buy/build a Teensy IONOS simulator (< USD 200) and run the exact IONOS matrix (WGN/MPG/MPP, 0–30 dB, bytes/min) | evidence-gap | H | M | winlink.org IONOS study methodology, pp. 23–25 |
| 10 | Add bytes/min-under-ARQ (net of retries) to `coppa-bench` and publish in IONOS format with CIs | evidence-gap | H | S | Every ham comparison uses B/min (IONOS; Mercury MODES.md goodput) |
| 11 | Resolve the SNR-reference ambiguity in BENCHMARKS.md: strike or relabel every pre-3 kHz-ref table; state "SNR in 3 kHz" on every published number | evidence-gap | H | S | BENCHMARKS.md has both a 0 dB and a 12 dB BPSK-1/2 FER≤10% table; coppa-channel lib.rs:98 |
| 12 | Rewrite README status table to match SPEC (OFDM working, 10 levels, LDPC NR BG2, IR-HARQ, VARA-TCP, KISS/AFSK, WebSocket); add a 30-second "what it is / isn't" | positioning | H | S | README says OFDM "Partial", QPSK+ "not wired", 9 levels; SPEC.md §5 lists 10 |
| 13 | Gate `vhf_wide` (350–5900 Hz) off for any HF frequency in the daemon; make speed levels 5–10 use an HF-legal profile on HF | regulatory | H | S–M | 47 CFR 97.307(f)(3) 2.8 kHz; CLAUDE.md: `select_ofdm_profile` routes levels ≥5 to `vhf_wide()` |
| 14 | Document band-plan compliance per profile: FCC 2.8 kHz OK; IARU R1 2700 Hz segments → `hf_wide` (2800 Hz) flagged non-R1; Canada 6 kHz allows `vhf_wide` on HF | regulatory | M | S | 97.307(f); IARU R1 HF band plan; RBR-4 |
| 15 | Implement CWID (Morse ID at session end / 10-min timer) and expose `CWID ON/OFF`; document in-band ID as §97.119(b)(3)-compliant | regulatory | M | S | 47 CFR 97.119; VARA has CWID; Mercury lacks it |
| 16 | Ship a "Message Viewer"-style offline decoder (`coppa rx --decompress --dump`) and document compression as a publicly specified code | regulatory | M | S | 97.309(a)(4); Winlink Open Letter on compression ≠ encryption |
| 17 | Web GUI served by `coppad`: waterfall, constellation, SNR/level/BUFFER, connect/listen buttons, TUNE, log | missing-feature | H | M–L | ardopcf webgui; VARA gauges; Mercury's stated gap ("no user interface") |
| 18 | `coppad --setup` wizard (sound device, PTT method, rigctld, callsign, BW) writing `coppad.toml` | missing-feature | M | S | VARA's single settings dialog; Direwolf commented config |
| 19 | Finish PTT: serial DTR/RTS, CM108/CM119 GPIO (Digirig/AIOC/DRA), Linux GPIO, hamlib/rigctld; document Digirig-Lite and AIOC cables | missing-feature | H | M | README: "Serial/GPIO PTT: Stub"; Direwolf PTT matrix; RadioMail's Digirig-Lite/AIOC support |
| 20 | BUSY-channel detect exposed on TCP/WebSocket and honoured before CONNECT | missing-feature | M | S | VARA/ARDOP busy detectors; IONOS busy-detect manual; spectrum sensing exists in `coppa-ml` |
| 21 | AGWPE port next to KISS for the AFSK TNC (Winlink Express Packet, Outpost, UI-View expect AGW) | ecosystem | M | M | Direwolf README: KISS + AGW |
| 22 | KISS-over-TCP datagram/broadcast mode on the OFDM HF modem for Reticulum; publish `docs/RETICULUM.md` | ecosystem | M | M | Reticulum manual (KISS/TCP `kiss_framing`); Mercury RETICULUM.md verified |
| 23 | One-to-many broadcast file mode (no ARQ, FEC + repetition) à la hermes-broadcast | missing-feature | M | M | Rhizomatica hermes-broadcast; FreeDATA 0.18 broadcast |
| 24 | Beacon → PSKReporter (or own map) reporter; decode others' beacons | ecosystem | M | S–M | VarAC PSKReporter self-report; FreeDATA statistics.freedata.app |
| 25 | A Linux/mac chat client (web) with VarAC semantics: calling freq + slots + auto-QSY via rigctld, ping, beacon, SNR list, VMail | missing-feature | M | L | VarAC feature list; "VarAC is a nightmare at Linux" [forum] |
| 26 | Publish crates to crates.io with docs.rs, semver, and 5 runnable examples; PyO3 bindings | positioning | M | M | No competitor is a library; codec2 is C |
| 27 | Session-robustness first: make the `session` bench pass zero drops on Good/Moderate before advertising bps | evidence-gap | H | L | IONOS: "NEVER dropping a connection" decided the comparison; CLAUDE.md: 3/5 Good, 0/5 Mod/Poor drop-free |
| 28 | Publish `docs/MODES.md`: per-level bandwidth, raw/net bps, SNR@FER≤10% (AWGN/Good/Mod/Poor, 3 kHz-ref), frame time | positioning | M | S | Mercury docs/MODES.md; codec2 README_data |
| 29 | Standing "scoreboard" page auto-generated from `coppa-bench` in CI, versioned per release | positioning | M | M | Gap analysis §6; no open modem publishes CI-driven curves |
| 30 | Low-rate ALE/beacon waveform (FT8/JS8-class, −15 to −20 dB, ~50-bit payloads) for connect-probe and spotting | missing-feature | M | L | JS8Call −24 dB; VARA L1 18 bps; gap analysis §5 |
| 31 | 500 Hz narrow mode parity: make `hf_narrow` (8+2 carriers) a first-class BW500 with its own bench row | missing-feature | M | M | IONOS 500 Hz results; IARU R1 500 Hz segments; VARA 500 |
| 32 | VHF FM data profile validated on a 9600-capable radio (VARA-FM-Wide-class) | missing-feature | M | L | IONOS VHF: VARA FM ≈140 kB/min vs packet ≈5 kB/min |
| 33 | Windows: signed installer, WASAPI/DirectSound device names, and Winlink Express "external VARA-compatible modem" recipe | platform | H | M | Windows share of Winlink/VarAC users is dominant (all Winlink Express/VarAC docs) |
| 34 | macOS: Apple-Silicon binary + `brew`; Pat on Mac is a real audience (pat-users M1 thread) | platform | M | S | pat-users VARA thread (M1 Wine issues) |
| 35 | Systemd unit, `--health` endpoint, watchdog restart, log rotation for 24/7 gateways | platform | M | S | K6AQ gateway talk ("must remain up as long/frequently as possible") |
| 36 | Getting-started that is *one command* per OS, verified in CI (docker) | platform | M | S | ardopcf "no explicit installation"; FreeDATA's PyPI "not yet working" as anti-example |
| 37 | Community surface: groups.io or Discussions, a `#coppa` channel, monthly release notes; announce on Zero Retries / HamWeekly / Newsline once #1, #8 exist | ecosystem | M | S | Mercury's launch coverage (Newsline, ICQ, HamWeekly, ARRL, Zero Retries) |
| 38 | Interop-test suite against `pat-vara`, LinBPQ, VarAC in CI using a virtual audio loop | ecosystem | M | M | No host app has ever driven coppa [repo] |
| 39 | Document the honest competitive table (this file's §3) in `docs/` and keep it updated; say plainly where coppa loses | positioning | M | S | Winlink Open Letter norms; Mercury's numberless parity claim is a cautionary tale |
| 40 | Name and tagline: "coppa" is invisible next to "VARA replacement"; pick a positioning line (e.g. "open, measurable, Linux-first HF modem") and use it everywhere | positioning | L | S | Zero Retries 0250 on discoverability; Mercury's "VARA replacement" framing |
| 41 | AFSK TNC: reproducible WA8LMF TNC Test CD decode count published | evidence-gap | L | S | Direwolf's "1000+ frames from Track 2" benchmark |
| 42 | iOS/Android reach: expose everything RadioMail/WoAD need (SNR, remote call, connect state) over the TCP API and document a "coppa on a Pi, RadioMail on the phone" recipe | ecosystem | L | S | RadioMail pairs to a VARA on a separate computer via Wi-Fi |

---

## 7. Sources (URLs actually read in this pass)

Repo (read-only): `/Users/thagale/Code/coppa/README.md`, `CLAUDE.md`, `docs/SPEC.md`, `BENCHMARKS.md`,
`docs/OPERATING.md`, `docs/analysis/2026-07-03-world-class-gap-analysis.md`, `crates/coppa-host/src/*`,
`crates/coppa-daemon/src/tnc.rs`, `crates/coppa-channel/src/lib.rs`, `.github/workflows/ci.yml`.

External:
- https://rosmodem.wordpress.com/ (VARA versions, tagline)
- https://www.sigidwiki.com/wiki/VARA_HF (levels, carriers, rates, licence)
- https://groups.io/g/VARA-MODEM/topic/vara_registration_hf_fm/80440399 and https://ema.arrl.org/2022/04/05/vara-hf-modem-group-license-purchase/ (licence terms, via search summary)
- http://wa8lmf.net/VARA/ (free-tier limit, host apps)
- https://winlink.org/sites/default/files/downloads/a_winlink_digital_mode_performance_comparison_based_on_the_ionis_sim_hf_vhf_channel_simulator_-_november_2_2020_0.pdf (IONOS study, all 25 pages read)
- https://winlink.org/content/winlink_express_and_vara_linux_mac_updated_instructions
- https://winlink.org/content/ardop_and_vara_now_beta_testing_winlink_software
- https://winlink.org/content/open_letter_all_digital_mode_stakeholders
- https://winlink.org/sites/default/files/RMSE_FORMS/winlink_gateways_on_the_cheap.pdf (K6AQ, 14 pages)
- https://github.com/pflarue/ardop , https://github.com/pflarue/ardop/blob/master/README.md , https://github.com/pflarue/ardop/blob/master/docs/Motivation.md , https://github.com/pflarue/ardop/releases
- https://ardop.groups.io/g/users/topic/ardop2_status/28508094 (via search summary; direct fetch 404)
- https://dl2man.de/ARDOP/ (browser ARDOP Winlink client)
- https://github.com/DJ2LS/FreeDATA , https://wiki.freedata.app/
- https://github.com/Rhizomatica/mercury , https://github.com/Rhizomatica/mercury/blob/mercuryv2/README.md , https://github.com/Rhizomatica/mercury/blob/mercuryv2/docs/MODES.md , https://github.com/Rhizomatica/mercury/blob/mercuryv2/docs/TNC.md
- https://www.rhizomatica.org/rhizomatica-releases-mercury-a-fully-open-source-modem-for-data-communications-on-hf/
- https://www.arnewsline.org/news-text/2026/5/14/open-source-software-modem-called-a-vara-replacement
- https://daily.hamweekly.com/2026/05/rhizomatica-releases-mercury-open-source-modem-digital-hf/
- https://www.w3ach.com/news?id=28 , http://www.arrl.org/mercury
- https://www.zeroretries.org/p/zero-retries-0250
- https://github.com/drowe67/codec2/blob/main/README_data.md (FreeDV data-mode table)
- https://getpat.io/ , https://github.com/n8jja/pat-vara , https://github.com/la5nta/wl2k-go/tree/master/transport
- https://groups.google.com/g/pat-users/c/F8qXAtRL1Aw (Pat VARA beta thread)
- https://github.com/WheezyE/Winelink/blob/main/docs/README.md
- https://eindhoven.space/2022/12/18/report-vara-hf-vms-rpi4-issues/
- https://www.varac-hamradio.com/ ; VarAC Pi-5/Wine forum thread (via search summary; direct fetch 402) ; VarAC V5.3.1 release notes (via search summary)
- https://github.com/wb2osz/direwolf
- https://themodernham.com/the-hard-truth-about-hardware-tncs-in-packet-radio/
- https://js8call.com/
- https://reticulum.network/manual/interfaces.html
- https://en.wikipedia.org/wiki/M17_(amateur_radio)
- https://winlink-portable.readthedocs.io/en/latest/radiomail/ ; https://winlink.org/content/radiomail_ios (via search summary)
- https://sailmail.com/ and https://www.cruiserswiki.org/wiki/Email_at_Sea (via search summary: SailMail USD 275/yr, PACTOR required)
- https://www.cruisersforum.com/forums/f64/dr-7400-p4dragon-pactor-4-modem-274001.html (used P4 price, via search summary)
- https://www.law.cornell.edu/cfr/text/47/97.307 , https://www.law.cornell.edu/cfr/text/47/97.309 , https://www.law.cornell.edu/cfr/text/47/97.119
- https://docs.fcc.gov/public/attachments/FCC-23-93A1.pdf and https://www.federalregister.gov/documents/2023/12/07/2023-26770/... (via search summary)
- https://www.iaru-r1.org/wp-content/uploads/2019/08/hf_r1_bandplan.pdf (via search summary)
- https://ised-isde.canada.ca/.../rbr-4-standards-operation-radio-stations-amateur-radio-service (via search summary)

Not reached (fetch failed or budget): varamodem.com (TLS), qsl.net/uz7ho (404), landfallnavigation.com (403),
Meshtastic/NPR/QRadioLink primary pages.
