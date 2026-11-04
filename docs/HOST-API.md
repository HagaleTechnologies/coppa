# Connecting a host application to coppad

This guide is for anyone writing a program that drives `coppad`: a Winlink client such as Pat, a
dashboard, a script, or your own host. It lists every TCP interface coppad serves, what each
one accepts, what it sends back, and where coppa's VARA-style port differs from VARA. To set up
Pat itself, read [PAT.md](PAT.md) first; it links back here for the protocol details.

coppa is **not** RF- or waveform-compatible with VARA or ARDOP. The VARA-style and ARDOP ports
copy those modems' TCP control interfaces so existing hosts can drive coppa. Both ends of a
radio link must run coppa, and both stations need coppad 0.3.0 or later
([OPERATING.md](OPERATING.md#connected-sessions-delivery-and-buffer)).

Every table below was checked against coppad 0.3.0 at commit `b9fdb8d` (see
[How this was verified](#how-this-was-verified)).

## Which interface to use

| Interface | Default port | Turn it on | Build feature | Authentication |
|---|---|---|---|---|
| VARA-style command port | 8300 | `[host] vara_enabled = true` | none, always built | none |
| VARA-style data port | 8301 | the same key | none | none |
| ARDOP command / data | 8515 / 8516 | `[host] ardop_enabled = true` | `ardop` | none |
| WebSocket JSON (and the `GET /` page) | 8400 | `[host] websocket_enabled = true` and `websocket_token` | `websocket` | token; browsers also need an allowed `Origin` |
| KISS (AFSK 1200 TNC mode) | 8001 | `coppad --tnc`, or `coppa tnc --port N` | `kiss-tnc` (and `cpal-backend` for audio) | none |

- Every `[host]` server is off by default and binds `bind_address`, which defaults to
  `127.0.0.1`. The keys and their defaults are in [coppad.toml.example](../coppad.toml.example);
  `coppad --print-default-config` prints them too.
- `cargo build -p coppa-daemon` with no features has the VARA-style ports only. A config that
  enables WebSocket or ARDOP in a build without that feature stops coppad at startup with an
  error naming the feature, for example `[host] websocket_enabled = true is set, but this coppad
  was built without the websocket feature`.
- KISS is a separate mode, not a `[host]` key. It runs the AFSK 1200 AX.25 TNC, not the OFDM
  modem, and has no session. See [KISS](#kiss-afsk-1200-tnc-mode).
- Pick the VARA-style ports for Pat's `varahf` transport or any VARA host; ARDOP for Pat's
  `ardop` transport or an ARDOP host; WebSocket for a dashboard or a program that wants JSON.
  All three drive the same modem and its one session.

## Try it with nc

**Security first.** The VARA-style ports have no authentication. Any program on this machine,
and a web page in your own browser (with an HTTP `POST` whose body is a command line), can key
your transmitter through them. Keep `bind_address = "127.0.0.1"`, and do not browse untrusted
sites on a machine whose coppad has `vara_enabled = true`
([OPERATING.md](OPERATING.md#what-this-does-not-cover)).

To try it with no radio or sound card, build coppad without `cpal-backend`
(`cargo build -p coppa-daemon --bin coppad`). It moves no audio but still answers commands and
runs the keying steps. (A `cpal-backend` build stops at startup if it finds no sound card.)
Start it with a config file holding only this:

```toml
[host]
vara_enabled = true
```

coppad warns that no callsign is configured; the `MYCALL` below sets one.

Then send a few commands. The `sleep`s give coppad time to answer each one:

```text
$ { echo VERSION; sleep 1; echo "MYCALL N1CALL"; sleep 1; echo "LISTEN ON"; sleep 1;
    echo FOOBAR; sleep 1; echo "CONNECT N1CALL N0CALL"; sleep 35; } \
    | nc -q 1 127.0.0.1 8300 | tr '\r' '\n'
VERSION Coppa 0.3.0
VERSION Coppa 0.3.0
OK
OK
WRONG
OK
PTT ON
PTT OFF
DISCONNECTED
```

- The first line is the greeting coppad sends as soon as you connect. The second answers
  `VERSION`.
- `OK` answers `MYCALL N1CALL` and `LISTEN ON`. `WRONG` answers `FOOBAR`, which coppad does not
  know. `OK` answers `CONNECT`.
- `PTT ON` and `PTT OFF` bracket the connect request coppad keys. `DISCONNECTED` arrives about
  30 s later because no station answered.
- `tr '\r' '\n'` is there because coppad ends every line it sends with a bare `\r`. Without it a
  terminal draws each reply over the last one.
- `-q 1` is the flag of the OpenBSD netcat that Debian, Ubuntu and Raspberry Pi OS install. The
  macOS `nc` has no `-q`; leave it out and press Ctrl-C after `DISCONNECTED`.

To type commands by hand, run `nc 127.0.0.1 8300 | tr '\r' '\n'` and type the same lines.
Enter sends `\n`, which coppad accepts. Commands are case-insensitive.

In a second terminal, `nc 127.0.0.1 8301` opens the data port. Type `hello` and press Enter:
nothing is sent, because no session exists, and coppad logs
`No established session; host data dropped, not transmitted`. coppad does not close an idle
data connection, so press Ctrl-C to leave.

With two coppads (two machines, or one machine with two audio devices; [PAT.md](PAT.md) shows
the configs), run `LISTEN ON` on one and `CONNECT <me> <other>` on the other, then type on
either data port. The [two-station example](#a-two-station-session) shows what both hosts see.

## The VARA-style command port

**Framing.** Send one command per line, ended by `\r`, `\n` or `\r\n`. Empty lines are ignored.
coppad ends every line it sends with a bare `\r`, as VARA does. On connect coppad speaks first,
with `VERSION Coppa 0.3.0`. A line longer than 4096 bytes, or one that is not valid UTF-8,
closes the connection.

**Replies and pushed lines.** Each command gets exactly one reply, sent only to the client that
sent it. Every other line coppad sends (`PTT`, `CONNECTED`, `BUFFER`, ...) goes to every
command-port client, with one exception: the `DISCONNECTED` that refuses a `CONNECT` because a
session already exists goes only to the client that dialled. Replies stay in the order you sent
the commands, but they can queue behind a transmission, so wait for each reply instead of counting
on a fixed delay.

### Commands

| You send | coppad replies | Effect |
|---|---|---|
| `VERSION` | `VERSION Coppa 0.3.0` | none |
| `MYCALL <call>` | `OK`; `WRONG` if the callsign is missing or invalid | sets the daemon's one callsign; only the first callsign is read |
| `LISTEN ON` / `LISTEN OFF` | `OK`; `WRONG` for any other argument | answer inbound calls, or stop; coppad starts with listening off |
| `CONNECT <me> <target> [via ...]` | `OK`, then `CONNECTED <caller> <callee> <bw>` or `DISCONNECTED` | dials `<target>`; `<me>` and `via` are ignored, the daemon's own callsign is used; one argument gets `WRONG` |
| `DISCONNECT` | `OK`, then `DISCONNECTED` once queued data is acknowledged | ends the session; with no session, `OK` and nothing else |
| `ABORT` | `OK`, then `DISCONNECTED` at once | ends the session or a pending dial, drops queued data, sends no DISCONNECT frame; with no session, `OK` and nothing else |
| `BW500`, `BW2300`, `BW2750`, `COMPRESSION <anything>` | `OK` | none; `[engine] profile` sets the waveform |
| `TUNE [seconds]` | `OK`, then `PTT ON`, `PTT OFF` | coppa extension: two-tone calibration, default 10 s; still `OK` when coppad cannot transmit (it logs why) |
| `CWID ON` / `CWID OFF` | `OK`; `WRONG` when `[station_id] cwid` locks the state (`"always"` refuses `OFF`, `"never"` refuses `ON`), or for any other argument | turns the closing Morse ID on or off for the whole daemon; the ID follows the DISCONNECT frame of a session this station ends; [OPERATING.md](OPERATING.md#morse-cw-id-at-session-end) has the exceptions |
| `FREQ` / `FREQ <hz>` | `FREQ <hz>`, a fresh reading from the rig; `WRONG` with no rigctld backend, no clean answer, or a refused set | coppa extension, needs `[radio] ptt_method = "rigctld"`: reads the rig's frequency; `<hz>` (decimal Hz) sets it first. Sets are off unless `[radio] host_rig_control = true`; [OPERATING.md](OPERATING.md#rig-frequency-and-mode-from-a-host-freq-mode) lists every refusal |
| `MODE` / `MODE <name>` | `MODE <token>`, the rig's own token (`USB`, `PKTLSB`, `CW`, ...); `WRONG` as for `FREQ` | coppa extension: reads the rig's mode; `<name>` is `USB`, `LSB`, `PKTUSB` or `PKTLSB` (any case) and sets it first under the same rules as `FREQ <hz>`; any other name gets `WRONG` |
| anything else, including `PUBLIC`, `CHAT`, `CQFRAME`, `WINLINK SESSION`, `P2P SESSION`, `CLEANTXBUFFER`, `BW9999` | `WRONG` | none |

- A `CONNECT` to a valid `<target>` while a session exists (connecting or connected) gets `OK`
  and then `DISCONNECTED`, to the dialling client only. Send `DISCONNECT` first.
- A `CONNECT` with no valid callsign set, or with an invalid `<target>`, gets `OK` and then
  `DISCONNECTED` at once. That `DISCONNECTED` goes to every command-port client, and every
  WebSocket client gets a `session` message with `"state":"disconnected"`. An invalid `<target>`
  is refused this way even while a session exists, and that session stays up.
- When the session is a pending inbound attempt (after `PENDING`), `DISCONNECT` and `ABORT` push
  `CANCELPENDING` before `DISCONNECTED`.
- `DISCONNECT` sends what the host already queued, waits for each frame's ACK, and only then
  ends the session ([OPERATING.md](OPERATING.md#ending-a-session-disconnect)).

### Lines coppad pushes

| Line | When |
|---|---|
| `VERSION Coppa <version>` | on connect, and in reply to `VERSION` |
| `PENDING` | a listening coppad answered another station's connect request; ends in `CONNECTED` or `CANCELPENDING` ([OPERATING.md](OPERATING.md#inbound-connect-attempts)) |
| `CANCELPENDING` | that inbound attempt ended without `CONNECTED` |
| `CONNECTED <caller> <callee> <bw>` | a session is up, on both stations; the caller is always first; `<bw>` is the profile's occupied bandwidth in Hz (2400 for `HF_STANDARD`) |
| `DISCONNECTED` | the session ended, or a dial failed or was refused |
| `PTT ON` / `PTT OFF` | at each PTT edge, for every transmission, ACKs included |
| `BUFFER <n>` | bytes the other station has not yet acknowledged; falls on the peer's ACK, not on PTT release ([OPERATING.md](OPERATING.md#connected-sessions-delivery-and-buffer)) |
| `BUSY ON` / `BUSY OFF` | channel occupancy changed and held for 250 ms ([OPERATING.md](OPERATING.md#busy-lines-on-the-command-port)) |
| `SNR <n>` | after each decoded frame |
| `IAMALIVE` | every 60 s, from one daemon-wide timer |

### Limits and timing

- 16 clients at once on each port. A 17th connection is closed with no greeting.
- coppad queues up to 1024 lines for each client. Once 64 are waiting it drops `BUSY` and `SNR`
  for that client (the next change sends a fresh value). At 1024 it closes the connection
  ([OPERATING.md](OPERATING.md#host-clients-that-stop-reading)). Every other line reaches a
  client or ends its connection.
- An unanswered `CONNECT` ends in `DISCONNECTED` about 30 s after the connect request is keyed.
- On the listening side, `PENDING` arrives first and `CONNECTED` a second or two after the
  dialler's `CONNECTED`. Do not expect `CONNECTED` first.

## How coppa differs from VARA

This list compares coppa only with the lines of EA5HVK's "VARA Protocol Native TNC Commands"
(February 2022) quoted in [the 2026-09-05 host-API review](reviews/2026-09-05-dim-C-host-apis.md).
Nobody has run coppa against real VARA. For a command that document does not cover (`PUBLIC`,
`CWID`, `CLEANTXBUFFER`), the table above says what coppa does and claims nothing about VARA.

- **The waveform.** Only the TCP interface is VARA-style. A VARA station cannot hear coppa.
- **`MYCALL`.** VARA documents up to five callsigns (`MYCALL Call1 ... Call5`). coppa reads the
  first and ignores the rest, and the callsign is daemon-wide, not per client.
- **`CONNECT Source Destination`.** coppa ignores `Source` and dials from the daemon's callsign.
- **`BW500|BW2300|BW2750` and `COMPRESSION OFF|TEXT|FILES`.** Both get `OK`, as in VARA, but
  change nothing. `[engine] profile` chooses the waveform, so match it on both stations.
- **`CHAT ON|OFF`, `CQFRAME`, `WINLINK SESSION`, `P2P SESSION`.** coppa answers `WRONG` and does
  nothing. Pat treats `WRONG` as a no-op.
- **`CONNECTED Source Destination BW`.** coppa sends the same three fields; `BW` is the coppa
  profile's occupied bandwidth in Hz (2400 for `HF_STANDARD`, 5600 for `VHF_FAST`), not 500, 2300
  or 2750.
- **Signal reports.** VARA documents `SN value`, sent only with `CHAT ON`. coppa never sends it;
  it sends its own `SNR <n>` after each decoded frame.
- **Lines coppa never sends:** `REGISTERED`, `LINK REGISTERED|UNREGISTERED`,
  `MISSING SOUNDCARD`, `CQFRAME`.
- **`TUNE [seconds]`** is a coppa extension for setting TX level.

Where the document and coppa agree: `DISCONNECT` waits for the TX buffer to empty, `ABORT` is a
dirty disconnect, `IAMALIVE` comes every 60 s, `BUFFER` falls when bytes are acknowledged,
`PENDING`/`CANCELPENDING`, `BUSY ON|OFF`, `PTT ON|OFF`, and `OK`/`WRONG` replies.

## The data port

- **Raw bytes.** No greeting, no framing, no escaping. A client that connects and reads gets
  nothing until data arrives. (The ARDOP data port, by contrast, is length-prefixed; see
  [ARDOP.md](ARDOP.md#the-data-port).)
- **A write is not a frame.** coppad reads up to 4096 bytes at a time and adds them to the
  session's queue. It cuts the queue into frames that fit the current speed level and sends one
  at a time, each acknowledged by the other station. The command port's `BUFFER` shows the
  bytes not yet acknowledged.
- **No session, no transmission.** Bytes written when no session is established are dropped,
  and coppad logs `No established session; host data dropped, not transmitted`. Setting
  `[host] sessionless_tx_enabled = true` sends them as sessionless frames instead, which lets
  any data-port client key the transmitter.
- **Received data goes to everyone.** Every data-port client and every WebSocket client (as a
  `data` message) gets each received byte once, in order.
- **Connections are not paired.** coppad has one session. Command-port and data-port
  connections are independent, and bytes from any data client join that one session.
- **Slow readers.** coppad keeps every received payload. A client with 1024 payloads waiting is
  disconnected; its command connection stays up
  ([OPERATING.md](OPERATING.md#host-clients-that-stop-reading)).

### A two-station session

Captured with a raw-socket driver on both stations' command and data ports, two release
coppads on the repo's userspace audio loop (`tools/pat-loop`), at `b9fdb8d`. Station A is
N1CALL and dials; station B is N0CALL and has sent `LISTEN ON`. Times are seconds from the
start; `BUSY` and `SNR` lines are left out.

```text
1.7  A -> CONNECT N1CALL N0CALL      1.7  A <- OK     1.8  A <- PTT ON
4.7  B <- PENDING                    4.8  A <- PTT OFF
8.1  A <- CONNECTED N1CALL N0CALL 2400     10.1 B <- CONNECTED N1CALL N0CALL 2400
11.2 A (data) -> "Hello N0CALL, this is N1CALL\r"     11.2 A <- BUFFER 29
12.8 B (data) <- "Hello N0CALL, this is N1CALL\r"     14.8 A <- BUFFER 0   (after B's ACK)
16.9 B (data) -> "73 N1CALL from N0CALL\r"            16.9 B <- BUFFER 22
18.5 A (data) <- "73 N1CALL from N0CALL\r"            20.5 B <- BUFFER 0
20.5 A -> DISCONNECT     20.5 A <- OK     20.7 A <- DISCONNECTED     22.2 B <- DISCONNECTED
```

## Session rules for every interface

- **One session at a time.** While one exists, a dial from any host is refused and other
  stations' connect requests go unanswered
  ([OPERATING.md](OPERATING.md#connected-sessions-delivery-and-buffer)). An idle session does
  not time out; a host must send `DISCONNECT`.
- **Listening starts off.** Only VARA `LISTEN ON` or ARDOP `LISTEN TRUE` turns it on. There is
  no config key and no WebSocket request for it. Until a host turns it on, coppad ignores every
  connect request.
- **VARA and ARDOP share the session.** A dial from one surface is refused while the other
  holds the link ([ARDOP.md](ARDOP.md#busy-semantics-one-transmitter-two-interfaces)).
- **Delivery.** Connected-session data is acknowledged and resent if lost; the other host gets
  every byte once, in order. Once a frame has used its 5 retransmissions, coppad holds the link
  for as long as the other station stays decodable, and ends the session only after 180 s with
  no sign of it ([OPERATING.md](OPERATING.md#connected-sessions-delivery-and-buffer)). A held
  session shows a `BUFFER` count that does not move.
- **Ending.** `DISCONNECT` drains the queue first; `ABORT` does not
  ([OPERATING.md](OPERATING.md#ending-a-session-disconnect)).
- **Inbound attempts.** `PENDING`, then `CONNECTED` or `CANCELPENDING`
  ([OPERATING.md](OPERATING.md#inbound-connect-attempts)).

## The WebSocket API

The WebSocket port speaks JSON text frames, one message per frame. The message types are
generated from the code into [schema/websocket-protocol-v1.json](schema/websocket-protocol-v1.json),
and a unit test keeps the two in step. Setting the token and the allowed origins is covered in
[OPERATING.md](OPERATING.md#websocket-access); this section covers what a client sees.

### Handshake

Present the token as `Authorization: Bearer <token>` or as `?token=<token>` in the URL (browser
JavaScript can only use the query form). The path is not checked.

| Request | Response |
|---|---|
| Upgrade with no token, or a wrong one | `401 Unauthorized`, `www-authenticate: Bearer` |
| Upgrade with the token given twice, once wrong (`?token=<good>&token=bad`) | `401` |
| Upgrade with the right token and an `Origin` not in `websocket_allowed_origins` | `403 Forbidden` |
| Upgrade with the right token and no `Origin` (a script) | `101 Switching Protocols` |
| Plain `GET /` or `GET /index.html` | `200` with the operator page, no token needed |
| Plain `GET /nope` | `404 Not Found` |
| Plain `POST /` | `405 Method Not Allowed` |

After `101` the first frame is:

```json
{"type":"hello","version":1,"server":"coppa 0.3.0"}
```

### Requests

Any request may carry an unsigned integer `id`. A request with an `id` gets exactly one reply
with that `id`: `ack`, `status` or `error`. An `ack` means accepted, not done. A request without
an `id` gets no `ack`. A `status` request is still answered and a rejected request still gets an
`error`, both without an `id`; every other request is accepted silently.

| Request | Reply | Notes |
|---|---|---|
| `{"type":"mycall","callsign":"N1CALL","id":1}` | `{"type":"ack","id":1}` | becomes the line `MYCALL N1CALL`; the ack does not mean the callsign is valid (coppad checks it later and only logs a warning) |
| `{"type":"connect","source":"N1CALL","destination":"N0CALL","id":2}` | `ack` | `source` is ignored; the outcome arrives as a `session` message, except when the dial is refused because another session already holds the link: that refusal sends a WebSocket client nothing, so check `status` |
| `{"type":"disconnect","id":3}` | `ack` | then `session` with `"state":"disconnected"` if a session existed; with none, only the `ack` |
| `{"type":"send","data":"hello","id":4}` | `ack` | `encoding` is `utf8` (the default) or `base64`; bad base64 gets `error`; with no session the bytes are dropped after the ack |
| `{"type":"status","id":5}` | `{"type":"status","id":5,"connected":false,"cp_desync_episodes":0}` | `remote_call` appears while a session exists; `snr`, `level` and `cfo` describe the last decoded frame and stay after a session ends; `short_cp_ok` appears when `[engine] cp_gate_enabled` is on |
| `{"type":"spectrum","enabled":true,"id":6}` | `ack` | opt in to (or, with `false`, out of) `spectrum` messages |
| `{"type":"audio_level","enabled":true,"id":7}` | `ack` | the same for `audio_level` |
| `{"type":"log","enabled":true,"id":8}` | `ack` | the same for `log` |

Anything else gets an `error`, with the request's `id` if it had one, for example
`{"type":"error","message":"Invalid message: unknown variant ...","id":9}`. Binary frames are
ignored. There is **no `listen` request**: a station driven only over WebSocket can dial but
cannot answer a call (see [Session rules](#session-rules-for-every-interface) and
[LIMITATIONS.md](LIMITATIONS.md)). A dial refused because another session holds the link is
also silent on WebSocket, as noted in the `connect` row.

### Messages coppad sends

| Message | When | Delivery |
|---|---|---|
| `hello` | first frame after the handshake | direct |
| `ack`, `status`, `error` | replies to requests | direct |
| `session` | `"state":"connected"` with `remote_call`, or `"state":"disconnected"` | reliable |
| `data` | received session data, with `encoding` `utf8` or `base64`, every byte kept | reliable |
| `ptt`, `busy`, `snr`, `level` | PTT edges, channel occupancy, SNR in dB, speed level | reliable |
| `heard` | a full snapshot of up to 32 recent session peers | reliable |
| `spectrum`, `audio_level`, `log` | only after the client opts in | lossy |
| `decode_fail` | a cumulative count, at most every 500 ms | lossy |

Part of a two-station session driven only over WebSocket: two release coppads on the repo's
audio loop at `b9fdb8d`, with `ptt`, `busy`, `snr`, `level` and `heard` messages left out. B had
sent `LISTEN ON` on its VARA command port first, since WebSocket cannot.

```text
A -> {"type":"connect","source":"N1CALL","destination":"N0CALL","id":3}
A <- {"type":"ack","id":3}
A <- {"type":"session","state":"connected","remote_call":"N0CALL"}
B <- {"type":"session","state":"connected","remote_call":"N1CALL"}
A -> {"type":"send","data":"Hello N0CALL over WebSocket","id":4}     A <- {"type":"ack","id":4}
B <- {"type":"data","data":"Hello N0CALL over WebSocket","encoding":"utf8"}
A -> {"type":"disconnect","id":7}   A <- {"type":"ack","id":7}
A <- {"type":"session","state":"disconnected"}   B <- {"type":"session","state":"disconnected"}
```

### Delivery and limits

- **Reliable** messages go through a queue of 256 for each client. A client that lets it fill is
  disconnected; it never silently misses a state or data message. After a reconnect, send
  `status`.
- **Lossy** messages share a 64-message broadcast channel that skips them for a slow client.
- `busy` and `snr` are reliable here but may be dropped on the VARA command port.
- 16 clients at once; 1 MiB per message; 256 KiB per frame.

The field-by-field description, with more examples, is in
[OPERATING.md](OPERATING.md#dashboard-protocol-v1).

## KISS (AFSK 1200 TNC mode)

`coppad --tnc` (always port 8001) or `coppa tnc --port N` runs a 1200-baud AFSK AX.25 TNC behind
a KISS TCP port. It is for bring-up and testing, not an interface to the OFDM modem, and Pat's
VARA path does not use it. [TNC.md](TNC.md) covers building and running it.

- Standard KISS framing: `FEND` (`0xC0`) around each frame, `FESC TFEND` and `FESC TFESC` for
  escaped bytes.
- Only command byte `0x00` (port 0, data) carries frames. TXDELAY, persistence and slot time
  are parsed and ignored.
- Send frames without an FCS. The TNC adds it on transmit, checks and strips it on receive.
- Up to 256 bytes of information field.
- Every client gets each decoded frame, unless it falls more than 64 frames behind: then it
  silently skips the oldest. Up to 16 clients.
- A client that sends more than 64 KiB with no `FEND` is disconnected.
- No authentication. Keep it on loopback.

TCP read boundaries do not delimit KISS frames. A frame is forwarded only after its closing
`FEND` arrives, including when an escape sequence spans reads (COP-148). Unfinished input at
EOF is discarded. The pending-byte cap is 64 KiB, excluding the retained opening delimiter;
exceeding it closes the connection.

**Known defect** (see [LIMITATIONS.md](LIMITATIONS.md)):

- **Frames on a KISS port other than 0 are dropped**, even when they are data frames. A host
  that numbers its ports from 1 sends nothing.

## ARDOP and the C FFI

- **ARDOP:** command port 8515, length-prefixed data port 8516, its own command subset and busy
  rules. See [ARDOP.md](ARDOP.md).
- **C FFI:** to embed the modem in your own program instead of talking to coppad, link
  `coppa-ffi` (a cdylib and staticlib with streaming decode). The header is
  [crates/coppa-ffi/coppa.h](../crates/coppa-ffi/coppa.h); the
  [getting-started tutorial](tutorials/getting-started.md) has a C example.

## Security

- The VARA-style, ARDOP and KISS ports have **no authentication**. Anything that can open a TCP
  connection to them can key your transmitter. `bind_address` defaults to `127.0.0.1`; change it
  only on a trusted, firewalled network.
- A web page in your own browser can still send a command line to the VARA-style command port
  in the body of a no-CORS `POST`, and coppad runs it. Until that is fixed, do not browse
  untrusted sites on a machine whose coppad has `vara_enabled = true`
  ([LIMITATIONS.md](LIMITATIONS.md), [OPERATING.md](OPERATING.md#what-this-does-not-cover)).
- The WebSocket port needs the token on every `bind_address`. Over `ws://` on a non-loopback
  address the token crosses the network in clear text.

## How this was verified

At commit `b9fdb8d` (coppad 0.3.0), on Linux, with no radio, no sound card and no RF:

- The `nc` walkthrough ran with OpenBSD netcat 1.226 against a debug coppad; its output above
  is verbatim. The same bytes, sent from Python sockets to a featureless debug coppad started
  with only the two-line config above, gave the same output. Writing `hello` to the data port
  with no session logged the drop.
- Every command-table row was sent over a raw socket and its reply recorded, including the
  16-client limit and the 4096-byte line limit. The `CQFRAME`, `COMPRESSION FILES`, two-callsign
  `MYCALL` and `BW500`/`BW2750` rows were re-checked against a debug coppad during COP-76.
- The two-station transcript and the WebSocket session came from two release coppads on the
  repo's userspace audio loop (`tools/pat-loop`), driven by raw-socket and WebSocket clients.
- Every handshake row came from a hand-written WebSocket client.
- The KISS split-frame and port-number defects were reproduced by feeding `KissServer` directly.
  The KISS audio paths were not exercised.
- Real Pat v1.0.0 completed Winlink sessions in both directions through the harness
  ([testing/pat-session.md](testing/pat-session.md#cop-76-run-record-2026-11-02)).

Not run: real VARA, ARDOP at this commit, the C FFI, real radios, a real browser.
