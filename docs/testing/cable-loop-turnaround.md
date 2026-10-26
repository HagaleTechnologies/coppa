# Cable-loop turnaround acceptance (H-090 stage d)

These are the pass criteria for the hardware cable-loop run of COP-60's
RX→TX turnaround (`[radio] turnaround_ms`, see `docs/OPERATING.md`). COP-20
executes them. CI runs a simulated version (see "CI counterpart" below).

## Setup

- Two hosts, two sound cards, TRS cables both ways, no radio.
- Both run `coppad` with `[engine] arq_enabled = true`,
  `[host] sessionless_tx_enabled = true`, and defaults otherwise
  (`turnaround_ms = 500`, `ptt_tail_delay_ms = 200`).
- A serial or GPIO PTT backend on **both** stations, its line wired to a scope
  or logic analyser. `ptt_method = "none"` leaves both PTT edges unobservable.
- Space single-segment host writes at least 5 s apart. A sender with several
  segments queued re-keys the moment its own PTT releases, so until
  sender-side turn discipline lands, back-to-back segments can collide with
  the peer's ACK regardless of this guard.

## Measure, per exchange

- `t_last`: the data frame's last audio sample on the cable.
- `t_peer_off`: the sending station's PTT line release.
- `t_ack_on`: the ACK station's PTT line assert.

## Pass: every exchange, at least 20 exchanges

- **Minimum guard:** `t_ack_on − t_peer_off ≥ 50 ms`.
- **Maximum ceiling:** `t_ack_on − t_last ≤ turnaround_ms + 250 ms`.
- **Delivery:** the sender's `BUFFER` returns to 0, i.e. the ACK was heard.

Publish min/median/max of both differences, with the WAVs, under
`results/ota/<date>/`.

## Why both bounds

A ceiling-only check passes a daemon with no turnaround at all. Measured in the
simulator with the guard disabled: the ACK station keyed 20 ms after the data
frame's last sample, well inside any ceiling, and 345 ms **before** the
sender's PTT-off, so the ACK was lost (11 bytes left unacknowledged). Only the
minimum guard, measured against the peer's real PTT-off edge, catches that.

The review's original ceiling of ≤ 250 ms from last sample to PTT-on
(`docs/reviews/2026-09-05-dim-F-modem-fitness.md`, stage d) cannot coexist with
a real minimum guard: the peer releases at its last presented sample plus a
200 ms tail, which leaves a 50 ms window, narrower than the receiver's own
decode latency. The ceiling is therefore the deliberate turnaround plus the
review's original 250 ms processing budget.

## CI counterpart

`crates/coppa-daemon/src/event_loop.rs`, `mod cop60_turnaround`:

- `cable_loop_ack_keys_after_the_peers_real_ptt_off_edge`: two real
  `EventLoop::run()` loops over a simulated half-duplex cable, default config.
  Requires the minimum guard (≥ 50 ms after the peer's recorded PTT-off), a
  ceiling of `turnaround_ms + 100 ms` from the last sample (virtual time has no
  decode CPU), delivery (0 unacked bytes), and no input-ring overflow.
  Measured: 155 ms after the peer's PTT-off, 520 ms after its last sample.
- `cable_loop_zero_turnaround_passes_the_ceiling_but_fails_the_minimum_guard`:
  the same loop with `turnaround_ms = 0` passes the ceiling and fails the
  minimum guard, which shows the check has teeth.

The simulated peer has no output worker, so it releases PTT after its last
sample plus a `buffer_size` drain (~170 ms) plus its tail, later than
hardware's last presented sample plus tail. The simulator's minimum guard is
therefore the stricter of the two.
