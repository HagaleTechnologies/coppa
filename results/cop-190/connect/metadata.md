# COP-190/COP-196 connect-bar evidence

Paired connection trials at constant −6 dB AWGN, recorded by the COP-196
trials on top of COP-190's physical layer. These files are measurement
inputs and outputs, not facts about the commit carrying them.

## Outcome

- **Link adapter: 10 of 10** connected. Every trial reached both-ends
  Connected (matching identities, decoded traffic at both ends) at 35.5
  simulated seconds, through COP-195's negotiated floor session controller
  (merged to main in `76823db`). Bar met.
- **Daemon adapter: 0 of 10** connected. Every trial timed out at the 60 s
  deadline with both stations IDLE: the daemon runs its own session stack
  and still advertises `robust_floor=false`, so it never negotiates the
  floor. Per decision COP-197 (option B) this bar is explicitly unmet and
  re-filed as COP-202; the audit still checks the daemon recording for
  completeness but does not enforce its pass count until COP-202 lands and
  re-records it.

The first recording (both adapters 0/10, taken on COP-190 alone before
COP-195 existed) is superseded for the link adapter by the run below; the
daemon re-run on the same revision reproduced its 0/10 CSV byte for byte.
No seeds, thresholds, gains or deadlines were changed to alter any outcome.

## Replay commands

```sh
mkdir -p /tmp/cop196-connect
COPPA_FLOOR_CONNECT_OUT=/tmp/cop196-connect \
  cargo test --release -p coppa-engine --test floor_session \
  floor_link_connect_minus6 -- --ignored --nocapture
COPPA_FLOOR_CONNECT_OUT=/tmp/cop196-connect \
  cargo test --release -p coppa-daemon --lib \
  floor_daemon_connect_minus6 -- --ignored --nocapture
```

The exported CSVs reproduce the committed `link.csv` / `daemon.csv` byte
for byte (same seeds, same channel contract, same deadline). Default test
execution never writes these files; export is explicit and scoped to the
directory named by `COPPA_FLOOR_CONNECT_OUT`.

The ordinary (non-ignored) audit and negative controls:

```sh
cargo test -p coppa-engine --test floor_session
cargo test -p coppa-daemon --lib floor_
```

`floor_connection_recordings_are_complete` checks both committed CSVs
(exact header, exact seed set, one row per trial, −6 dB only, finite
in-range elapsed times, success/timeout time consistency, failure-reason
consistency) and requires at least nine connected rows for the link adapter (the daemon
pass count is reported but not enforced until COP-202). The muted negative controls
(`floor_link_muted_peers_never_connect`,
`floor_daemon_muted_peers_never_connect`) prove the trial runners cannot
report connection success from local intent alone.

## Configuration

- Dependency revision tested: COP-196 branch merged with main
  `76823db0e888d764dd23302cada71e5fe0895666` (includes COP-190 and
  COP-195's library floor session controller). The daemon floor slice is
  absent at this revision (COP-202).
- Rust: rustc 1.98.0 (88d9e12ae 2026-08-18), cargo 1.98.0
  (797e8a9bc 2026-08-05); first recording on Linux, current link and
  daemon re-runs on macOS (aarch64).
- Adapters: engine library `Link` pairs (caller VK3ABC, listener W1AW,
  `LinkConfig::new` defaults: level 1, compression off, 500 ms turnaround,
  5 s initial RTO, 3 connect retries) and daemon `EventLoop` pairs
  (`DaemonConfig::default()`, listening on, zero PTT pre/tail delay,
  default 500 ms turnaround, dial through `HostEvent::ConnectRequest`).
- Sample rate 48 kHz, one common sample clock, 960-sample blocks (20 ms).
- Channel: `PersistentChannel` AWGN only, `SnrProfile::constant(-6.0)` on
  the channel's existing 3 kHz reference, no fading. Direction 0 is
  A→B, direction 1 is B→A, each seeded per the persistent-channel
  contract from the run seed.
- Seeds: ten trials, indices 0–9, `0x16910000 + index`, identical seed
  set for both adapters, fresh Links/stations and fresh channels per
  trial, persistent state throughout each trial.
- Reference power: clean active mean-square power of the production floor
  waveform (`CoppaCore::new().floor().encode(b"connect bar reference")`)
  at its transmitted amplitude, measured once per run before the trials;
  the same value feeds every channel. Never estimated from silence or
  from the noisy output. Measured value on these runs: **0.500000**
  (identical for both adapters; the trials print it at startup).
- Deadline: 60 simulated seconds (2,880,000 samples) from the initial
  connect request, including initial OFDM attempts; success requires both
  peers confirmed (both Connected, matching peer identities, decoded
  handshake traffic at both ends) within the window. A timeout records
  60.0 s and the terminal states.
- Half-duplex: `Link::process` ignores rx while transmitting; the daemon
  polls its audio-in ring per block. Each peer hears the previous tick's
  channel output, so neither side gets an instantaneous response.

## Separate pass counts

- `link.csv`: 10/10 connected (seeds 0x16910000–0x16910009).
- `daemon.csv`: 0/10 connected (same seeds); bar re-filed as COP-202.

All ten outcomes per adapter are retained, failures included. Superseded
run: the initial link recording on COP-190 alone (pre-COP-195), 0/10, all
timeouts with both ends IDLE and nothing decoded.
