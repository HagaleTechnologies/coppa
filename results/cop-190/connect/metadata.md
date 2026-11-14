# COP-190/COP-196 connect-bar evidence

Paired connection trials at constant −6 dB AWGN, recorded by the COP-196
trials on top of COP-190's physical layer. These files are measurement
inputs and outputs, not facts about the commit carrying them.

## Outcome

Both adapters recorded **0 of 10** connected trials. Every trial timed out
at the 60 s deadline with both stations IDLE (the caller's dial exhausted
its retries after nothing decoded) and zero decoded frames at either end.
The root cause is the missing negotiated floor session controller tracked
by COP-195: at −6 dB the OFDM handshake legs do not decode, and no
floor-carried session path exists yet to carry them. These trials are the
recorded proof of that gap; they are expected to turn green once COP-195's
controller is incorporated. No seeds, thresholds, gains or deadlines were
changed to alter the outcome.

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
consistency) and requires at least nine connected rows per adapter, so it
fails until the bar is actually met. The muted negative controls
(`floor_link_muted_peers_never_connect`,
`floor_daemon_muted_peers_never_connect`) prove the trial runners cannot
report connection success from local intent alone.

## Configuration

- Dependency revision tested: COP-190 head `355805ccc999803fbdffc6a9bbe5bca17604c2e8`
  merged onto main `cdc4ae5f852634dda4f289744a8050ffc8d61179` (COP-196
  branch). COP-195's controller is absent at this revision.
- Rust: rustc 1.98.0 (88d9e12ae 2026-08-18), cargo 1.98.0
  (797e8a9bc 2026-08-05), Linux.
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

- `link.csv`: 0/10 connected (seeds 0x16910000–0x16910009).
- `daemon.csv`: 0/10 connected (same seeds).

All ten outcomes per adapter are retained, failures included; there are
no superseded runs to record (these are the first recorded runs under
this contract).
