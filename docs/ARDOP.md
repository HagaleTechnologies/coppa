# The ARDOP host interface (COP-97)

`coppad` can expose an **ARDOP TNC host interface**: a second two-socket TCP
surface (command port + data port) speaking the ARDOP host protocol that
Pat (via `wl2k-go/transport/ardop`) and Winlink Express drive.

**This is a host *protocol* interface, not a waveform.** Over the air coppa
speaks coppa: an ARDOP TNC cannot complete an RF session with `coppad`, and
both ends of a `coppad` session must run coppa. The ARDOP ports exist so a
host that only speaks ARDOP can drive coppa — the same reason the
VARA-style ports exist (`docs/PAT.md`).

## Configuration

```toml
[host]
bind_address = "127.0.0.1"
# VARA-style interface (default off)
vara_enabled = false
vara_command_port = 8300
vara_data_port = 8301
# ARDOP TNC host interface (COP-97, default off). Data = command + 1,
# which is how every ARDOP client derives it.
ardop_enabled = true
ardop_command_port = 8515
ardop_data_port = 8516
```

Build with the `ardop` feature (`cargo build -p coppa-daemon --features
ardop`); a config that sets `ardop_enabled = true` in a build without the
feature is a startup error. Defaults when the keys are absent: disabled,
ports 8515/8516 (ardopcf's defaults).

**No token.** Like the VARA ports (COP-53), the ARDOP ports have no
authentication. Any local process, or anyone who can reach a non-loopback
listener, can key the transmitter through them. A browser page can also
submit command lines using a no-CORS request, the cross-protocol risk
recorded in COP-53. This interface adds another unauthenticated command
port to that existing risk. Keep
`bind_address = "127.0.0.1"` unless the network is trusted and firewalled.

## Pat configuration

```json
{
  "mycall": "N1CALL",
  "locator": "FN31PR",
  "ardop": { "addr": "127.0.0.1:8515" }
}
```

Pat derives the data port by ASCII-incrementing the last character of the
command address (`8515` → `8516`), which is why the data port must be
command + 1, and the command port must not end in `9`. Set `mycall` and
`locator` to your own callsign and grid square. Then:

```sh
pat connect ardop:///N0CALL      # dial a station
pat --listen=ardop               # listen for inbound sessions
```

## The command port

No greeting on connect; the client speaks first; lines are `\r`-terminated
(`\n` and `\r\n` are tolerated). Accepted commands are acked by **echoing
the command back**; a malformed line answers `WRONG`; a semantically failing
command answers `FAULT <text>`.

Unsolicited notifications: `NEWSTATE <state> ` (note the trailing space,
preserved for byte-compatibility with real TNCs), `TARGET <local-call>` (a pending
inbound request — **required before `CONNECTED` for Pat's listen mode**),
`CONNECTED <call> <bw>`, `DISCONNECTED`, `BUSY TRUE|FALSE`, `PTT TRUE|FALSE`,
`BUFFER <n>` (unacked host bytes; falls on the peer's ACK, not on PTT
release).

### Command subset

| Verified: Pat sends it | Accepted and echoed | Answered `WRONG` |
|---|---|---|
| `INITIALIZE`, `STATE`, `CODEC`, `PROTOCOLMODE`, `ARQTIMEOUT`, `LISTEN`, `MYCALL`, `GRIDSQUARE`, `ARQBW`, `VERSION`, `ARQCALL`, `DISCONNECT`, `CANCELPENDING`, `CWID`, `SENDID` | `SENDVER`, `ARQCALLBYNAME`, `GRACETIMERS`/`GRACETIMES`, `SENDBUFFER`, `FECSEND`, `MYAUX`/`MYAUXCALL`, `TRIMBLE` | Every other verb or malformed command |

Verified: **Pat sends it** (read from `wl2k-go/transport/ardop`'s init and
session flow):

| Command | Behaviour |
|---|---|
| `INITIALIZE` | echo; marks the TNC initialized (`STATE` leaves `OFFLINE`) |
| `STATE` | `STATE <word>`: `OFFLINE` before initialization/no callsign, `DISC` when disconnected, `ISS`/`IRS` while connecting; an established link reports `ISS` |
| `CODEC TRUE/FALSE` | echo (coppa models no codec gate) |
| `PROTOCOLMODE ARQ` | echo |
| `ARQTIMEOUT <s>` | echo, recorded — **does not change coppa's retry budget** (COP-38) |
| `LISTEN TRUE/FALSE` | echo, applied (answers inbound connects) |
| `MYCALL [<call>]` | set: echo, applied to the station-ID gate; query: `MYCALL <call>`; invalid → `WRONG` |
| `GRIDSQUARE <grid>` | echo, stored, unused by ARQ |
| `ARQBW [<bw>]` | set: echo, stored per client as a label; query: `ARQBW <bw>` |
| `VERSION` | `VERSION Coppa <version>` (honest: Pat displays it in logs) |
| `ARQCALL <target> [<repeats>]` | the dial: no echo; `NEWSTATE ISS ` then `CONNECTED <target> <label>`, or `FAULT busy` + `NEWSTATE DISC ` when another session holds the link |
| `DISCONNECT` | echo, then `DISCONNECTED` + `NEWSTATE DISC ` |
| `CANCELPENDING` | echo; drops a pending (not yet established) attempt |
| `CWID TRUE/FALSE` | echo (coppa keys no Morse ID) |
| `SENDID` | echo; transmits a real station-ID frame — `FAULT` with no callsign set or when the channel is busy |

Accepted and echoed (real-TNC commands no verified client sends; Winlink
Express is closed source, so a spurious `WRONG` would break its init):
`SENDVER`, `ARQCALLBYNAME`, `GRACETIMERS`/`GRACETIMES`, `SENDBUFFER`,
`FECSEND`, `MYAUX`, `MYAUXCALL`, `TRIMBLE`. No semantics are emulated.

Anything else answers `WRONG`.

### Honestly not modelled

- **`ARQBW` does not change coppa's speed ladder.** The `CONNECTED` line's
  bandwidth token echoes the client's stored `ARQBW` (default `500MAX`) as a
  label. Match `profile` on both stations, as `docs/PAT.md` teaches for
  VARA.
- **`ARQTIMEOUT` does not change coppa's retry budget** (COP-38's
  retransmit rules govern).
- **No FEC mode** (`FECSEND` is echo-ack only).

## The data port

Frames both directions (verified against Pat's own client source, not
inferred):

- host → TNC: `<len:u16 big-endian><payload>`, `len` = payload length.
- TNC → host: `<len:u16 big-endian><tag:3 ASCII><payload>`, `len` =
  3 + payload length, tag `ARQ` for session data.

Example round trip (hex): the host sends `00 04 64 65 6d 6f` ("demo"); the
TNC answers `00 07 41 52 51 64 65 6d 6f` (len=7, `ARQ`, "demo"). Data
written with no established session is dropped by the COP-29 gate (logged,
never transmitted).

## Busy semantics (one transmitter, two interfaces)

The VARA-style and ARDOP interfaces drive the same modem and its one
session, so they cannot both drive the transmitter: while a session is
active, a dial from the *other* surface is refused. A refused `ARQCALL`
answers `FAULT busy` + a client-scoped `NEWSTATE DISC ` — the
vocabulary Pat already treats as refusal (the ARDOP protocol has no
dedicated busy token); a refused VARA `CONNECT` keeps its client-scoped
`DISCONNECTED`. The refused ARDOP client's `STATE` remains `STATE DISC`. The first
session never hears either refusal. The same
refusal applies between two clients on the ARDOP surface itself.

## Wire compatibility evidence

Pat v1.0.0 uses wl2k-go v1.0.1. Its [get loop](https://github.com/la5nta/wl2k-go/blob/v1.0.1/transport/ardop/tnc.go)
waits for a reply with the requested command name. Consequently `STATE`
answers `STATE DISC`, not a bare state word. Its [state map](https://github.com/la5nta/wl2k-go/blob/v1.0.1/transport/ardop/ardop.go)
recognizes `DISC`, `ISS`, and `IRS`; connection success is the separate
`CONNECTED <remote> <label>` notification. New-state notifications use
`NEWSTATE ISS ` for the caller, `NEWSTATE IRS ` for the listener, and
`NEWSTATE DISC ` at teardown. `TARGET` names the local called address,
which Pat's [listener](https://github.com/la5nta/wl2k-go/blob/v1.0.1/transport/ardop/listen.go)
uses as the accepted connection's local address.

`DISCONNECT` is idempotent. A repeated close receives its echo followed by
`DISCONNECTED` and `NEWSTATE DISC ` even after the link has ended. Stored
`GRIDSQUARE`, `CODEC`, `PROTOCOLMODE`, `ARQTIMEOUT`, and `CWID` settings may
also be queried by sending the verb alone after setting a value.
