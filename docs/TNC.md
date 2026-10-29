# KISS TNC (AFSK 1200)

## What this mode is for

`coppa tnc` runs a 1200-baud AFSK (Bell 202) AX.25 TNC behind a KISS TCP port. Use it for
KISS bring-up and testing: checking that a KISS client, your audio routing and your PTT reach
coppa. It is not a Direwolf replacement. It has no AGWPE interface, no channel-busy check (it
ignores KISS TXDELAY, persistence and slot time), no digipeating or beaconing, and runs only at
1200 baud. That parity work is planned as H-122 in the
[2026-09-05 product review](reviews/2026-09-05-product-ux-market-review.md).

What it does:

- Serves KISS over TCP on `127.0.0.1`, port 8001 by default (`--port` changes it).
- Sends and receives AX.25 frames with up to 256 bytes of information field. The KISS client
  builds the frames; the TNC does not parse them.
- Modulates and demodulates Bell 202 (1200 Hz mark, 2200 Hz space) at 48 kHz.
- Keys PTT through rigctld (`--rig localhost:4532`), or not at all.

What it does not do:

- No AGWPE interface. Clients that only speak AGW (Winlink Express packet, Outpost, UI-View)
  cannot use it.
- Only KISS port 0 carries data. A data frame addressed to any other KISS port is dropped.
- KISS TXDELAY, persistence and slot time are accepted and ignored. There is no carrier sense:
  the TNC transmits as soon as a frame arrives.
- No digipeating, no beaconing and no configuration file.
- `--vox` keys nothing. It behaves like no PTT, so the radio's own VOX has to key it.
- 1200 baud only.
- No error correction beyond the AX.25 frame check sequence (FCS). A frame with a bad FCS is
  discarded.

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
 INFO TNC mode kiss_port=8001 preferred_hardware_rate=48000 engine_rate=48000
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

- Frames carry no FCS, as the KISS protocol specifies. The TNC adds the FCS on transmit, and
  on receive it checks the FCS, drops frames that fail, and strips it before passing the frame
  to clients. A host that appends its own FCS gets two on air.
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
