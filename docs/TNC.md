# KISS TNC (AFSK 1200 and G3RUH 9600)

For KISS datagrams on the HF OFDM modem, see [RETICULUM.md](RETICULUM.md).
This page covers the separate packet TNC mode.

## What this mode is for

`coppa tnc` runs an AX.25 TNC behind a KISS TCP port. It defaults to 1200-baud AFSK
(Bell 202); `--baud 9600` selects G3RUH. Use it for
KISS bring-up and testing: checking that a KISS client, your audio routing and your PTT reach
coppa. It is not a Direwolf replacement. Digipeating is opt-in with
`--digipeat CALL[-SSID]`; a spectral busy check gates automatic repeats. KISS client
transmissions still start immediately and ignore TXDELAY, persistence and slot time.
AGWPE is connectionless only. It has no beaconing. It runs at 1200 baud (AFSK) or 9600 baud (G3RUH). Remaining parity work
is planned as H-122 in the
[2026-09-05 product review](reviews/2026-09-05-product-ux-market-review.md).
Measured against Direwolf 1.7 on track 2 of the WA8LMF TNC Test CD, coppa decodes 412 packets
to Direwolf's 1000 (41.2 percent). The method and the script that reproduces it are in
[testing/tnc-direwolf-wa8lmf.md](testing/tnc-direwolf-wa8lmf.md).

What it does:

- Serves KISS over TCP on `127.0.0.1`, port 8001 by default (`--port` changes it).
- Sends ordinary AX.25 by default and automatically receives AX.25 and Direwolf 1.7 IL2P.
  The KISS client builds complete AX.25 frames without FCS; digipeating
  inspects only their address fields.
- `--il2p` selects maximum-FEC IL2P transmission with up to 1023 information bytes
  for losslessly representable two-address UI frames. Other frames use transparent
  encapsulation with a 1023-byte limit on the entire AX.25 frame.
  `--il2p` and `--fx25` are mutually exclusive.
- Modulates and demodulates Bell 202 (1200 Hz mark, 2200 Hz space) or G3RUH 9600 at 48 kHz.
  IL2P transmit and receive are 1200-baud only; `--il2p` is refused at 9600.
- Keys PTT through rigctld (`--rig localhost:4532`), or not at all.

What it does not do:

- No AX.25 connected sessions. Winlink Express packet and Outpost BBS messaging require
  a future link layer; UI-View interoperability has not been verified.
- Only KISS port 0 carries data. A data frame addressed to any other KISS port is dropped.
- KISS TXDELAY, persistence and slot time are accepted and ignored. Client-initiated
  transmissions start immediately. Only automatic repeats wait for a clear channel.
- No beaconing and no configuration file.
- `--vox` keys nothing. It behaves like no PTT, so the radio's own VOX has to key it.
- IL2P is 1200 baud only.
- Ordinary AX.25 uses FCS validation. IL2P uses Reed-Solomon correction and has no
  trailing CRC in the Direwolf 1.7 format. Some corruption beyond its correction
  bound can be delivered undetected. See [the measured coverage](testing/tnc-direwolf-il2p.md).

## 9600 baud (G3RUH)

Select `coppa tnc --baud 9600`. G3RUH uses direct FSK: connect the radio's
9600-capable discriminator input/output path. Microphone audio paths generally
filter out the required baseband signal. Device selection with `--device`,
`--input-device-id` and `--output-device-id` works the same way at both rates.

On the pinned Direwolf-generated 100-frame recording at 10 dB SNR, coppa delivers
100 frames to Direwolf's 100, meeting the 95 percent bar. See the
[measurement record](testing/tnc-direwolf-g3ruh-9600.md) for the fixture and method.
This synthetic result does not measure real radio performance or clock drift.

The KISS contract, PTT controls and ignored TXDELAY are unchanged. Both modems run
at a 48 kHz engine rate. The 9600 receiver merges five fixed sampling phases and
suppresses identical frames received within 150 ms; see
[LIMITATIONS.md](LIMITATIONS.md) for the timing limits.

## Digipeating

Enable exact callsign and SSID matching with an alias:

```bash
coppa tnc --rig localhost:4532 --digipeat N0CALL-2
```

The additional startup and repeat log lines are:

```text
 INFO Digipeat enabled alias=N0CALL-2 dedupe_window=30s busy_wait=5s
 INFO Digipeated W1AW>APRS via N0CALL-2
```

Digipeating is off unless the flag is given. Only the first unused path entry can
match. The repeat marks that entry used and recomputes FCS without changing the
rest of the frame. There is no WIDEn-N alias substitution or counter decrement.
All decoded frames still reach KISS clients, whether repeated or not.

Duplicate UI frames are suppressed for 30 s, ignoring path used bits and FCS. The cache
holds 64 frames. Connected-mode frames (SABM, I, S and other non-UI frames) are never
deduplicated, because their senders retransmit identical frames after a loss. One pending repeat is retained; a newer repeat replaces it. Every automatic
transmit checks the spectral channel-busy flag and available transmitter space. A
repeat waiting for a busy channel is dropped after 5 s, with a DEBUG log line:
`digipeat frame dropped: channel busy for 5s`.

The detector reuses the daemon's BusyGate, with roughly 0.25 s detection latency
on onset and release. The release delay provides a courtesy gap. Synthetic in-memory
probes measured modulated Bell 202 detection at full scale and 0.1 full scale,
a 5 s carrier staying busy throughout, and detection of a 400 ms burst.
These measurements used a digital-silence quiet floor. Real-channel behavior
on a physical radio is untested; they do not establish performance on a noisy band.

## Build

On Debian or Ubuntu, install the ALSA headers first:

```bash
sudo apt install libasound2-dev
```

Then build and install the `coppa` binary with both the TNC and live audio:

```bash
cargo install --path crates/coppa-cli --features cpal-backend,kiss-tnc
```

`coppa tnc` should log both `Audio … started` lines. It writes its status to stderr as
levelled log lines (coloured only on a terminal; `RUST_LOG` overrides the `info` default):

```text
$ coppa tnc --port 8001 --device coppa-loop-A
 INFO TNC mode kiss_port=8001 preferred_hardware_rate=48000 engine_rate=48000 modem=afsk_1200
 INFO KISS server listening port=8001
 INFO PTT: null (no PTT configured)
 INFO Audio input started input_hardware_rate=48000 engine_rate=48000
 INFO TNC ready.
 INFO Audio output started output_hardware_rate=48000 engine_rate=48000
```

If it prints `Audio: none (built without the cpal-backend feature; nothing will be sent or
received)` instead, the binary was built without `cpal-backend`. Rebuild with the command
above.

Use `coppa tnc`, not `coppad --tnc`, for this guide. `coppad --tnc` always listens on port
8001 and uses the default audio device, so two of them cannot run on one machine.

## IL2P with Direwolf

Start the same service with `coppa tnc --il2p --vox` to transmit IL2P with
16 parity bytes per payload block. Device, channel, port and PTT options work
as before. Both this command and the default `coppa tnc --vox` receive IL2P
automatically alongside ordinary AX.25. `coppad --tnc` also receives both,
while retaining ordinary AX.25 transmission.

KISS payloads are complete AX.25 frames without FCS in either mode. IL2P adds
neither AX.25 FCS nor HDLC stuffing. It receives both automatic and maximum
FEC modes, either tone polarity, and type-1 UI/I/S/U and transparent type-0
headers. TX uses type 1 for lossless UI headers and type 0 for other frames,
including digipeater paths. Invalid or oversized IL2P requests are dropped
before PTT with `TNC frame cannot be encoded as IL2P; dropping frame`.
They are never truncated or sent using a different encoding.

[Byte-level interoperability tests](testing/tnc-direwolf-il2p.md) cover clean
48 kHz audio with Direwolf 1.7. No noisy-channel or live RF result is claimed.

## Demo: two TNCs on one Linux machine

The demo runs two `coppa tnc` instances, A and B, joined by a virtual audio cable, and sends
APRS-style packets between them with Direwolf's `kissutil` as the KISS client. On Debian and
Ubuntu, `kissutil` comes with Direwolf (`sudo apt install direwolf`). Its default port, 8001,
is also coppa's.

1. Load the ALSA loopback driver. It adds a virtual sound card whose device 0 and device 1 are
   cross-wired: what one plays, the other captures.

   ```bash
   sudo modprobe snd-aloop
   ```

2. Give each end of the cable its own name. `--device` accepts a unique name
   substring; if several devices match, Coppa lists their IDs and refuses to
   choose one. For reliable selection, copy the exact input and output IDs
   from `coppa devices` into `--input-device-id` and `--output-device-id`:

   ```bash
   cat >> ~/.asoundrc <<'EOF'
   pcm.coppa_loop_a { type plug; slave.pcm "hw:Loopback,0,0"; hint { show on; description "coppa-loop-A" } }
   pcm.coppa_loop_b { type plug; slave.pcm "hw:Loopback,1,0"; hint { show on; description "coppa-loop-B" } }
   EOF
   ```

3. Check that coppa sees both names:

   ```text
   $ coppa devices
   Available audio devices:
     …
     coppa-loop-A [id: <A-id>] (in: …, out: …, max: … Hz)
     coppa-loop-B [id: <B-id>] (in: …, out: …, max: … Hz)
   ```

4. Start the two TNCs, each in its own terminal. Each prints the startup lines shown under
   [Build](#build) and ends with `TNC ready.` and `Audio output started`.

   ```bash
   coppa tnc --port 8001 --input-device-id "<A-id>" --output-device-id "<A-id>" # terminal 1
   coppa tnc --port 8002 --input-device-id "<B-id>" --output-device-id "<B-id>" # terminal 2
   ```

5. In a third terminal, attach a receiving client to TNC B. It prints each packet B decodes.

   ```bash
   kissutil -p 8002
   ```

6. In a fourth terminal, send two packets through TNC A. The first is an APRS position report.
   The second has a 256-byte information field, the largest AX.25 allows; at 1200 baud it is
   about 2 seconds on air, longer than the TNC's audio buffer holds at once, so it checks that
   a full-size frame is sent whole.

   ```bash
   echo 'N0CALL>APRS:!4903.50N/07201.75W-coppa KISS demo' | kissutil -p 8001
   printf 'N0CALL>APRS:>coppa KISS demo %0239d\n' 0 | kissutil -p 8001
   ```

   The client on B prints each packet exactly as sent. The second line is 272 characters: 239
   zeros follow `demo `.

   ```text
   [0] N0CALL>APRS:!4903.50N/07201.75W-coppa KISS demo
   [0] N0CALL>APRS:>coppa KISS demo 0000…0000
   ```

7. The other direction works the same way. Run `kissutil -p 8001` as the receiver and send
   through B:

   ```bash
   echo 'N1CALL>APRS:>reply from B' | kissutil -p 8002
   ```

   The client on A prints `[0] N1CALL>APRS:>reply from B`.

Stop each `kissutil` and TNC with Ctrl-C. To remove the virtual card afterwards, run
`sudo modprobe -r snd-aloop` and delete the two `coppa_loop` lines from `~/.asoundrc`.

IDs are specific to the audio host and can change if it reconfigures a device.
Each stream carries mono modem audio on one zero-based channel. Use
`--input-channel 1` and/or `--output-channel 1` for the second lane of a stereo
interface; other output lanes remain silent. If hardware cannot run at the
48 kHz modem rate, Coppa selects a supported rate and converts audio while
keeping the modem at 48 kHz. An invalid channel or stale ID fails before the
TNC reports ready.

## KISS details

See also [HOST-API.md](HOST-API.md#kiss-afsk-1200-tnc-mode), which lists these details for host
authors. The shared server now keeps a frame whole across split TCP reads, including
escapes. Nonzero KISS ports remain unsupported.

- Frames carry no FCS, as the KISS protocol specifies. For ordinary AX.25 the TNC
  adds FCS on transmit and checks and strips it on receive. For IL2P it passes
  the reconstructed AX.25 bytes unchanged. Hosts must not append their own FCS.
- Data travels on KISS port 0 only (command byte `0x00`).
- Every connected client receives every decoded frame. Up to 16 clients can connect at once.
- The server binds `127.0.0.1` and has no authentication: anything that can open a TCP
  connection on this machine can key the transmitter. See [SECURITY.md](../SECURITY.md).

## Troubleshooting

- **No `Audio … started` lines.** The binary has no live audio: it prints `Audio: none …`
  instead. Rebuild with `--features cpal-backend,kiss-tnc` (see [Build](#build)).
- **`WARNING: No input device matching 'coppa-loop-A', using default`.** No device has that
  name. Check that `~/.asoundrc` holds the two `coppa_loop` lines, that `snd-aloop` is loaded,
  and that `coppa devices` lists `coppa-loop-A` and `coppa-loop-B`.
- **An ambiguous name lists several IDs.** Copy the desired ID from `coppa devices`
  and use the corresponding directional ID flag. An explicit ID that is missing
  or has the wrong direction is an error; it does not use the default device.
- **The receiving `kissutil` prints nothing.** Check that both TNCs printed `TNC ready.` and
  both `Audio … started` lines, that the receiver is attached to the other TNC's port, and that
  the two exact device IDs are different.

## How this demo was verified

The original name-based `coppa` and `kissutil` demo was run with these ports
against a userspace stand-in for `snd-aloop` (ALSA `file` PCMs joined by a
paced relay). Exact ID selection and stereo channel wiring are covered by
deterministic tests; they still need checking on a physical radio interface.
The `modprobe` and `hw:Loopback` lines were not run in that container because
it had no sound modules.

## FX.25 forward error correction

Use `coppa tnc --fx25 --vox` to transmit FX.25. Reception automatically recognizes
ordinary AX.25 and all eleven defined FX.25 profiles with 16, 32 or 64 parity
bytes. Default transmission remains ordinary AX.25.

FX.25 transmission selects the shortest fitting block with 32 parity bytes,
using tags 8, 7, 6 or 5. The capacity is at most 223 data bytes including FCS,
bit stuffing and inner HDLC flags. It is smaller for payloads with many one bits.
An oversized KISS frame is sent intact as ordinary AX.25, with one warning:
`TNC frame exceeds FX.25 capacity; transmitting plain AX.25`.

The additive Rust APIs are `coppa_codec::afsk::modulate_fx25`, accepting a frame
including its FCS, and `coppa_daemon::tnc::run_tnc_with_fx25`. Existing `run_tnc`
and `TncConfig` callers keep their default behavior. Encoding rejects invalid FCS.
See [the interoperability record](testing/tnc-fx25-direwolf.md) for independent
Direwolf fixtures, correction tests and external decoder verification. These
controlled byte errors do not measure general noisy-audio or RF performance.

## Optional connectionless AGWPE

```sh
coppa tnc --port 8001 --agw-port 8000 --bind-address 127.0.0.1
# A second station on the same host uses distinct ports and audio devices:
coppa tnc --port 18001 --agw-port 18000 --device coppa-loop-B
```

AGWPE is optional. Start it with `--agw-port 8000` and point your client at
127.0.0.1, TCP port 8000, radio port 1. The wire protocol numbers that radio
port as 0. Port 8001 is KISS and cannot accept AGWPE requests.

The connectionless AGWPE subset supports callsign registration, UI-frame
transmission, raw reception and UI-frame monitoring. Winlink Express packet
sessions and Outpost BBS sessions require AX.25 connected mode, which is not
implemented. UI-View interoperability has not been verified with the real client.

These listeners have no authentication. Any client that can reach them can
request a transmission. The default bind address is 127.0.0.1. Use a LAN
address only on a trusted network. `--bind-address` changes both listeners.
A bind failure names the listener address and flag and exits before readiness.
A build without `cpal-backend` serves control requests but moves no audio.

See [the AGW wire contract](HOST-API.md#agwpe-connectionless-afsk-tnc) for supported
commands, limits, monitoring, queue counts and overload behavior.
Run `python3 tools/agw-probe.py --host 127.0.0.1 --port 8000` from the repository
root for a control-only check. It requests a transmission, without claiming delivery.
A TNC started with `--baud 9600` needs `--baud 9600` on the probe too, since the
`G` and `g` replies then describe a G3RUH 9600 port.

Automated verification: `cargo test -p coppa-daemon --features kiss-tnc --lib tnc::`
includes `agw_tcp_audio_round_trip`, which registers through TCP, transmits binary
and 256-byte UI information, demodulates simulated output, checks PTT release and
receives raw and monitor records through TCP. The CLI subprocess tests run with
`cargo test -p coppa-cli --features kiss-tnc --test tnc_agw` without CPAL.

Not run: Winlink Express, Outpost, UI-View, real sound hardware and RF.
For a future field check, start two stations with distinct ports and audio devices,
select AGWPE in UI-View with the first station's host, TCP port 8000 and radio port 1,
send a UI packet and check the peer's bytes and display, then send a reply. Record
client versions and packet captures. Connected-session checks require the future
AX.25 link layer and are not a supported workflow today.
