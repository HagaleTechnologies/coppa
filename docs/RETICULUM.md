# Reticulum on the HF OFDM modem

`coppad` can carry Reticulum packets over its OFDM modem through KISS over TCP.
One KISS data frame becomes one broadcast OFDM frame. There is no ACK, retry,
fragmentation or reassembly. Every peer KISS client receives the original bytes;
VARA-style, ARDOP and WebSocket data clients do not receive datagrams, and there
is no local echo. Both stations need a release that supports MAC Datagram type 9.

## Build and configure

```sh
cargo build --release -p coppa-daemon --features cpal-backend,kiss-tnc
```

Configure each station's audio devices and radio as in [OPERATING.md](OPERATING.md).
Add these keys to `coppad.toml`, using your own callsign:

```toml
[audio]
buffer_size = 131072

[engine]
callsign = "N0CALL"
profile = "HF_STANDARD"

[host]
bind_address = "127.0.0.1"
kiss_enabled = true
kiss_port = 8100
```

The larger audio ring absorbs live input during synchronous encoding. A run
with the default ring delivered both packets but logged dropped input during
encoding, so it did not pass the overflow check.

Start `coppad --config coppad.toml`. `kiss_enabled` defaults to false; `kiss_port`
defaults to 8100. A build without `kiss-tnc` refuses an enabled KISS config.
The startup log gives the actual listening address, speed level and maximum
packet size. An occupied port is a startup error.

In each node's `~/.reticulum/config`, add:

```ini
[interfaces]
  [[Coppa HF]]
    type = TCPClientInterface
    interface_enabled = True
    kiss_framing = True
    target_host = 127.0.0.1
    target_port = 8100
    fixed_mtu = 500
    bitrate = 600
```

`fixed_mtu = 500` prevents Links negotiating packets larger than the modem can
carry. The TCP interface otherwise supports much larger packets. Reticulum uses
`bitrate` to estimate timing and limit announces. Its default `announce_cap` is
2 percent; announce-rate tuning on HF has not been measured here.

## Capacity and airtime

The limit includes the entire Reticulum packet, not just application data. Each
frame carries 1 to 8 codewords, with a 14-byte MAC header and no compression.
`HF_STANDARD` starts at level 2. `HF_ROBUST` at level 1 drops packets over 434
bytes, so it cannot carry every Reticulum packet. Oversize packets are dropped
with a warning; KISS has no error reply.

| Level | Maximum datagram bytes | 167-byte airtime | 500-byte airtime | Suggested bitrate |
|---|---:|---|---|---:|
| 1 | 434 | 3.67 s | does not fit | 300 |
| 2 | 922 | 2.52 s | 5.99 s | 600 |
| 3 | 922 | 1.36 s | 3.10 s | 1200 |
| 4 | 1410 | 0.79 s | 1.94 s | 2000 |
| 5 | 1250 | 0.97 s | 1.73 s | 2200 |
| 6 | 922 | 0.79 s | 1.65 s | 2400 |
| 7 | 1410 | 0.50 s | 1.08 s | 3600 |
| 8 | 1250 | 0.58 s | 0.97 s | 4000 |

Airtimes are research measurements for uncompressed PHY payloads before the
14-byte MAC header. At these two packet sizes the header changes no codeword
count except level 4's 167-byte row, which now needs 2 codewords. The row is
retained as the measured baseline, not an exact Datagram-mode airtime. Suggested
bitrates round down `4000 / seconds` for 500 bytes, leaving some margin for PTT
and turnaround. Level 1 uses `1336 / 3.67` because 500 bytes does not fit.
Actual throughput also includes station identification, turnaround and contention.

## Transmission rules and security

A valid local callsign is required. The normal station-ID prefix, frequency gate,
busy gate, turnaround delay and TX watchdog apply. Carrier sense defaults off
(`[station_id] busy_hold_ms = 0`); the effect of enabling it on Reticulum has not
been measured. At most eight datagrams wait to transmit; the newest is dropped
when full. A datagram is refused during a session or handshake, and queued
packets are discarded if a session starts. Receive delivery remains available.

Keep sessionless ARQ disabled alongside this mode: a datagram can key between a
sessionless ARQ frame and its ACK when `sessionless_tx_enabled` and `arq_enabled`
are enabled together. Datagram mode does not negotiate a rate.

The port has no authentication. Anyone who can connect can key the transmitter.
Keep the loopback default or restrict access on a trusted, firewalled network.
Old coppa releases do not recognize MAC type 9 and hand the full frame, header
included, to their VARA-style and ARDOP data clients as raw data.

Reticulum data and Link traffic is end-to-end encrypted. Amateur rules in many
countries restrict messages encoded to obscure their meaning. In the US,
[47 CFR 97.113(a)(4)](https://www.ecfr.gov/current/title-47/chapter-I/subchapter-D/part-97/subpart-B/section-97.113)
restricts such messages, with exceptions stated in the rules. The current eCFR
text was checked on 2026-11-04. The operator must decide whether the planned
traffic is permitted; coppa does not make that decision.

## How this was verified

`crates/coppa-daemon/tests/kiss_datagram.rs` runs two real EventLoops and KISS
servers with real TCP clients on a half-duplex virtual audio cable. It checks a
403-byte packet, a full 500-byte packet both ways, TCP split-frame handling,
absence of local echo, and isolation from VARA data clients. Virtual time hides
real-time decoding stalls.

`tools/reticulum-loop/run.sh` runs real RNS 1.5.7 and two release coppads over the
userspace ALSA file cable. It requires no radio or sound card. The first run used the default audio ring. Both payloads arrived, but live input
overflowed during the first encode. It failed the harness overflow check. The
second run uses the larger audio ring shown in the setup above.

First run, verbatim stdout and stderr:

```text
[notice] A new release of pip is available: 25.0.1 -> 26.3
[notice] To update, run: python3 -m pip install --upgrade pip
reticulum-loop: RNS 1.5.7, coppad 0.7.0
reticulum-loop: scratch /tmp/reticulum-loop.Cq0sKB
peer->a RECEIVED DATAGRAM 300 bytes ok=True
peer->b RECEIVED DATAGRAM 300 bytes ok=True
Audio input buffer overflow
FAIL: logs kept at /tmp/reticulum-loop.Cq0sKB
```

Not verified: RF, real radios, fading, collisions with other stations, multi-hop
Reticulum transport, or Reticulum timers at HF round-trip times.

Second run, verbatim stdout and stderr:

```text
[notice] A new release of pip is available: 25.0.1 -> 26.3
[notice] To update, run: python3 -m pip install --upgrade pip
reticulum-loop: RNS 1.5.7, coppad 0.7.0
reticulum-loop: scratch /tmp/reticulum-loop.jqvYQp
peer->a RECEIVED DATAGRAM 300 bytes ok=True
peer->b RECEIVED DATAGRAM 300 bytes ok=True
PASS
```
