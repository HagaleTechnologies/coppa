> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-119: RS(12,6) session fade protection

Measured on 2026-11-06 with workspace version 0.8.0, on the COP-119 branch.
Command: `cargo run --release -p coppa-bench --example session`.
The original window-eight bare-frame baseline is unchanged. The new
`stop-and-wait` and `rs12-6` arms include MAC and transport headers, window
one, a 1200-sample RX tail, and independent channel seeds for every physical
transmission. They use the production outer packetizer, reassembler and ARQ.
Each fading summary row averages five trials, including dropped trials. These are simulated
ten-minute sessions with ideal ACKs unless explicitly labeled ACK-loss.
The channel is reset per packet, not a continuous on-air fading realization.
A retry that `get_retransmits` hands out is still permitted, so the framed
arms transmit it and give it its ACK opportunity before a drop is recorded,
matching the daemon's send-then-mark order (the 2026-11-04 run predated this
and recorded drops one retransmission early); dropped trials print the number
of sends of the segment that exhausted the budget.

The ACK-loss arm sends ACK frames through the same channel. Protected blocks
use two copies and the sender's conservative extra hold if only the second
copy decodes. See SPEC §22.4. This arm is reported separately from the
Moderate 5/5 acceptance policy. No seeds or retry constants were tuned.

Clean cable, using the two real daemon event loops and a deterministic
2000-byte host write, measured default `accept` at **2706.4 B/min** in
44.34 seconds and `request` at **1724.1 B/min** in 69.60 seconds. Protected
mode was 36.3% slower on this payload. The cable harness includes handshake,
framing, PTT and host delivery; the bench's clean row isolates steady-state
session transfers. The cable run took 115.20 seconds wall time and checked
that keyed intervals from the two stations did not overlap.

Reproduce the cable report with
`cargo test -p coppa-daemon --lib --features cpal-backend,websocket rs_cable_clean_throughput_report -- --ignored --nocapture`.

Measurement wall time: 511.2 seconds for the session executable, excluding its release build.

The protected ideal-ACK arm completed Good, Moderate and Poor at 5/5.
With ACK loss it completed Good, Moderate and Poor at 5/5.
This does not prove RF reliability or survival through a continuous channel.

Full session output:

```text
=== Session-robustness bench: 5 x 10-min ARQ sessions/preset ===
Level 2 (BPSK 1/2), window 8, SNR ramp 20->0->20 dB over the session.
Drop = ArqTx::is_failed (RTO cap reached), not a single retransmit.

  [good] trial 0: completed (602.7s, 15912 bytes, 1584.1 bytes/min)
  [good] trial 1: DROPPED (491.9s, 9126 bytes, 1113.1 bytes/min)
  [good] trial 2: completed (600.0s, 15795 bytes, 1579.4 bytes/min)
  [good] trial 3: DROPPED (449.5s, 8892 bytes, 1186.9 bytes/min)
  [good] trial 4: DROPPED (449.9s, 8307 bytes, 1107.9 bytes/min)
-> good: 2/5 sessions completed without a drop, avg net goodput 1314.3 bytes/min

  [moderate] trial 0: DROPPED (399.3s, 6903 bytes, 1037.2 bytes/min)
  [moderate] trial 1: DROPPED (403.8s, 7137 bytes, 1060.6 bytes/min)
  [moderate] trial 2: DROPPED (371.5s, 6669 bytes, 1077.0 bytes/min)
  [moderate] trial 3: DROPPED (408.8s, 7137 bytes, 1047.6 bytes/min)
  [moderate] trial 4: DROPPED (368.5s, 6552 bytes, 1066.8 bytes/min)
-> moderate: 0/5 sessions completed without a drop, avg net goodput 1057.8 bytes/min

  [poor] trial 0: DROPPED (293.3s, 1755 bytes, 359.0 bytes/min)
  [poor] trial 1: DROPPED (266.4s, 1638 bytes, 368.9 bytes/min)
  [poor] trial 2: DROPPED (246.4s, 1521 bytes, 370.4 bytes/min)
  [poor] trial 3: DROPPED (250.5s, 1404 bytes, 336.3 bytes/min)
  [poor] trial 4: DROPPED (279.8s, 1287 bytes, 276.0 bytes/min)
-> poor: 0/5 sessions completed without a drop, avg net goodput 342.1 bytes/min

=== Acceptance policy: Good deterministic regression floor ===
MET: Good completed 2/5 sessions without a drop (required at least 2/5).
Moderate and Poor are diagnostic-only; their results remain reported above.

=== COP-119 daemon-matched window-1 session arms ===
-> stop-and-wait ideal-ACK clean: 1901.0 bytes/min
  stop-and-wait ideal-ACK [good] trial 0: completed (601.9s, 12384 bytes, 1234.4 bytes/min)
  stop-and-wait ideal-ACK [good] trial 1: completed (601.7s, 7968 bytes, 794.5 bytes/min)
  stop-and-wait ideal-ACK [good] trial 2: DROPPED after 6 sends (402.4s, 6528 bytes, 973.4 bytes/min)
  stop-and-wait ideal-ACK [good] trial 3: DROPPED after 6 sends (325.6s, 6624 bytes, 1220.7 bytes/min)
  stop-and-wait ideal-ACK [good] trial 4: completed (600.7s, 13152 bytes, 1313.6 bytes/min)
-> stop-and-wait ideal-ACK good: 3/5 completed, 1107.3 bytes/min
  stop-and-wait ideal-ACK [moderate] trial 0: completed (602.5s, 10944 bytes, 1089.8 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 1: completed (601.6s, 12864 bytes, 1283.0 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 2: completed (601.5s, 11232 bytes, 1120.4 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 3: DROPPED after 6 sends (353.2s, 6336 bytes, 1076.3 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 4: completed (600.5s, 11328 bytes, 1131.8 bytes/min)
-> stop-and-wait ideal-ACK moderate: 4/5 completed, 1140.3 bytes/min
  stop-and-wait ideal-ACK [poor] trial 0: completed (600.0s, 1920 bytes, 192.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 1: completed (601.1s, 5280 bytes, 527.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 2: DROPPED after 6 sends (345.5s, 576 bytes, 100.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 3: completed (600.8s, 4896 bytes, 488.9 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 4: DROPPED after 6 sends (450.7s, 2976 bytes, 396.2 bytes/min)
-> stop-and-wait ideal-ACK poor: 3/5 completed, 340.8 bytes/min
-> stop-and-wait ACK-loss clean: 1901.0 bytes/min
  stop-and-wait ACK-loss [good] trial 0: DROPPED after 6 sends (341.5s, 5856 bytes, 1029.0 bytes/min)
  stop-and-wait ACK-loss [good] trial 1: completed (601.9s, 9024 bytes, 899.5 bytes/min)
  stop-and-wait ACK-loss [good] trial 2: completed (600.3s, 9888 bytes, 988.4 bytes/min)
  stop-and-wait ACK-loss [good] trial 3: DROPPED after 6 sends (341.6s, 5952 bytes, 1045.5 bytes/min)
  stop-and-wait ACK-loss [good] trial 4: DROPPED after 6 sends (325.8s, 6048 bytes, 1113.8 bytes/min)
-> stop-and-wait ACK-loss good: 2/5 completed, 1015.3 bytes/min
  stop-and-wait ACK-loss [moderate] trial 0: DROPPED after 6 sends (319.8s, 4896 bytes, 918.7 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 1: completed (600.0s, 8832 bytes, 883.2 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 2: DROPPED after 6 sends (324.8s, 4800 bytes, 886.6 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 3: DROPPED after 6 sends (285.8s, 5088 bytes, 1068.2 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 4: completed (600.1s, 9216 bytes, 921.5 bytes/min)
-> stop-and-wait ACK-loss moderate: 2/5 completed, 935.6 bytes/min
  stop-and-wait ACK-loss [poor] trial 0: completed (600.3s, 672 bytes, 67.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 1: DROPPED after 6 sends (451.9s, 672 bytes, 89.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 2: completed (600.3s, 672 bytes, 67.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 3: completed (600.2s, 2016 bytes, 201.5 bytes/min)
  stop-and-wait ACK-loss [poor] trial 4: DROPPED after 6 sends (175.5s, 1632 bytes, 558.1 bytes/min)
-> stop-and-wait ACK-loss poor: 3/5 completed, 196.6 bytes/min
-> rs12-6 ideal-ACK clean: 1814.5 bytes/min
  rs12-6 ideal-ACK [good] trial 0: completed (607.9s, 14675 bytes, 1448.5 bytes/min)
  rs12-6 ideal-ACK [good] trial 1: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [good] trial 2: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [good] trial 3: completed (610.9s, 14675 bytes, 1441.3 bytes/min)
  rs12-6 ideal-ACK [good] trial 4: completed (610.9s, 14675 bytes, 1441.3 bytes/min)
-> rs12-6 ideal-ACK good: 5/5 completed, 1518.5 bytes/min
  rs12-6 ideal-ACK [moderate] trial 0: completed (612.4s, 14088 bytes, 1380.4 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 1: completed (611.9s, 14088 bytes, 1381.5 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 2: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 3: completed (607.9s, 14675 bytes, 1448.5 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 4: completed (609.4s, 14675 bytes, 1444.9 bytes/min)
-> rs12-6 ideal-ACK moderate: 5/5 completed, 1457.2 bytes/min
  rs12-6 ideal-ACK [poor] trial 0: completed (610.2s, 9979 bytes, 981.1 bytes/min)
  rs12-6 ideal-ACK [poor] trial 1: completed (600.8s, 10566 bytes, 1055.2 bytes/min)
  rs12-6 ideal-ACK [poor] trial 2: completed (600.8s, 10566 bytes, 1055.2 bytes/min)
  rs12-6 ideal-ACK [poor] trial 3: completed (607.1s, 11153 bytes, 1102.3 bytes/min)
  rs12-6 ideal-ACK [poor] trial 4: completed (619.2s, 10566 bytes, 1023.9 bytes/min)
-> rs12-6 ideal-ACK poor: 5/5 completed, 1043.6 bytes/min
-> rs12-6 ACK-loss clean: 1814.5 bytes/min
  rs12-6 ACK-loss [good] trial 0: completed (613.8s, 14675 bytes, 1434.5 bytes/min)
  rs12-6 ACK-loss [good] trial 1: completed (600.9s, 12327 bytes, 1230.8 bytes/min)
  rs12-6 ACK-loss [good] trial 2: completed (613.3s, 14675 bytes, 1435.6 bytes/min)
  rs12-6 ACK-loss [good] trial 3: completed (612.3s, 14675 bytes, 1438.1 bytes/min)
  rs12-6 ACK-loss [good] trial 4: completed (611.8s, 14675 bytes, 1439.2 bytes/min)
-> rs12-6 ACK-loss good: 5/5 completed, 1395.7 bytes/min
  rs12-6 ACK-loss [moderate] trial 0: completed (602.0s, 14088 bytes, 1404.1 bytes/min)
  rs12-6 ACK-loss [moderate] trial 1: completed (606.1s, 13501 bytes, 1336.6 bytes/min)
  rs12-6 ACK-loss [moderate] trial 2: completed (615.0s, 14675 bytes, 1431.7 bytes/min)
  rs12-6 ACK-loss [moderate] trial 3: completed (605.4s, 13501 bytes, 1338.0 bytes/min)
  rs12-6 ACK-loss [moderate] trial 4: completed (609.1s, 14088 bytes, 1387.8 bytes/min)
-> rs12-6 ACK-loss moderate: 5/5 completed, 1379.6 bytes/min
  rs12-6 ACK-loss [poor] trial 0: completed (614.4s, 5283 bytes, 515.9 bytes/min)
  rs12-6 ACK-loss [poor] trial 1: completed (602.2s, 7631 bytes, 760.3 bytes/min)
  rs12-6 ACK-loss [poor] trial 2: completed (618.0s, 7044 bytes, 683.9 bytes/min)
  rs12-6 ACK-loss [poor] trial 3: completed (613.8s, 8805 bytes, 860.7 bytes/min)
  rs12-6 ACK-loss [poor] trial 4: completed (607.5s, 8805 bytes, 869.6 bytes/min)
-> rs12-6 ACK-loss poor: 5/5 completed, 738.1 bytes/min
MET: rs12-6 Moderate completed 5/5 sessions without a drop (required 5/5).
```

