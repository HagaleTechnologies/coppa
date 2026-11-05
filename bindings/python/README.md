# coppa Python binding

A standard-library-only (`ctypes`) binding for coppa's C library. It drives a
connected, reliable coppa link (`coppa_link_*`, the coppad session protocol
without the daemon) from Python. Added in COP-139.

Not published; see `docs/LIMITATIONS.md`. Tested on Linux with Python 3.12.

## Setup

Build the C library, then point Python at this directory:

```bash
cargo build -p coppa-ffi --release
export PYTHONPATH=bindings/python
```

The package finds the library in this order: `$COPPA_LIB`, then
`target/release`, then `target/debug` of this repository, then the system
search path. Set `COPPA_LIB` to the library's full path to choose one.

## Example

```python
import coppa

print(coppa.version(), coppa.abi_version())   # '0.5.0' (1, 0)

link = coppa.Link("N0AAA")                    # profile=None, speed_level=0, turnaround_ms=0
link.connect("N0BBB", now_ms=0)
rx = b""                                      # float32 samples captured since the last call
tx, active = link.process(20, rx, tx_len=960) # play tx; key the transmitter while active > 0
while (event := link.poll_event()) is not None:
    print(event.kind.name, event.remote, event.reason.name)
```

A failed call raises `coppa.CoppaError` with the library's message:

```text
coppa.CoppaError: [-7] coppa_link_send: link is not connected (state IDLE)
```

`Link` methods: `listen(on=True)`, `connect(remote, now_ms)`, `send(data)`,
`disconnect(now_ms)`, `process(now_ms, rx, tx_len=None)`, `recv(max_bytes=65536)`,
`poll_event()`, `close()`, and the context-manager protocol. Properties:
`state` and `pending_bytes`.

## Two processes over a socketpair

```bash
COPPA_LIB=target/release/libcoppa_ffi.so python3 bindings/python/examples/two_process_link.py
```

The parent makes a `socket.socketpair()` and starts two processes, one per
end. Each sends 960 float32 samples (20 ms) plus one status byte per tick and
reads the peer's block. The sender connects, sends 24 bytes and disconnects;
the listener prints the bytes it received.

## Rules

- **Time.** `now_ms` is a monotonic millisecond count from any epoch you
  choose. It must never decrease for one link; a smaller value raises
  `CoppaError` with code -6.
- **Threads.** A `Link` may be called from several threads (the library locks
  inside the handle). Feed one audio stream from one thread only, and call
  `close()` only after every other thread is done with the link.
- **Profiles.** Both ends must use the same profile (OFDM band and compression).
- **ABI.** Loading checks `coppa_abi_version()`. The binding needs ABI 1.x
  with minor 0 or newer. A library built before COP-139 has no
  `coppa_abi_version` and is refused as too old.

## Tests

```bash
cargo build -p coppa-ffi
COPPA_LIB=$PWD/target/debug/libcoppa_ffi.so python3 -m unittest discover -s bindings/python/tests -v
```
