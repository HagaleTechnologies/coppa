#!/usr/bin/env python3
"""Two OS processes hold a coppa link over a socketpair audio cable (COP-139).

Run with no arguments to be the parent: it creates a ``socket.socketpair()``,
starts two copies of this script (``--role send`` and ``--role listen``), one
per socket end, relays their output and exits non-zero if either fails.

Each child runs a lockstep loop. Every tick (20 ms of virtual time) it calls
``Link.process`` with the peer's previous block, writes one status byte and
960 little-endian float32 samples to its socket end, and reads the peer's
status byte and block. The status byte is 1 once that side is done, so both
stop on the first tick where both are done.

    COPPA_LIB=target/release/libcoppa_ffi.so python3 bindings/python/examples/two_process_link.py
"""

import argparse
import json
import os
import socket
import struct
import subprocess
import sys
from array import array
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import coppa  # noqa: E402

BLOCK = 960  # 20 ms at 48 kHz
TICK_MS = 20
MAX_TICKS = 6000  # 120 s of virtual time
SENDER = "N0AAA"
LISTENER = "N0BBB"
PAYLOAD = b"hello from process A\x00\xff\xfe\n"


def say(role, text):
    print(f"[{role} pid {os.getpid()}] {text}", flush=True)


def recv_exact(sock, n):
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise EOFError("peer closed the cable")
        buf.extend(chunk)
    return bytes(buf)


def run_child(role, fd, expect):
    sock = socket.socket(fileno=fd)
    is_sender = role == "sender"
    link = coppa.Link(SENDER if is_sender else LISTENER)
    received = bytearray()
    sent = False
    disconnected = False
    done = False
    peer_done = False
    rx = array("f", bytes(4 * BLOCK))
    if is_sender:
        say(role, f"connecting to {LISTENER}")
        link.connect(LISTENER, 0)
    else:
        link.listen(True)
        say(role, f"listening as {LISTENER}")
    for tick in range(1, MAX_TICKS + 1):
        now = tick * TICK_MS
        tx, _active = link.process(now, rx, BLOCK)
        while True:
            ev = link.poll_event()
            if ev is None:
                break
            if ev.kind == coppa.EventKind.CONNECTED:
                say(role, f"connected to {ev.remote}")
                if is_sender and not sent:
                    link.send(PAYLOAD)
                    sent = True
                    say(role, f"sent {len(PAYLOAD)} bytes")
            else:
                say(role, f"disconnected ({ev.reason.name.lower()})")
                disconnected = True
        data = link.recv()
        if data:
            received.extend(data)
            if len(received) >= expect:
                say(role, f"received {len(received)} bytes: {bytes(received).hex()}")
        if is_sender and sent and link.state == coppa.LinkState.CONNECTED and link.pending_bytes == 0:
            link.disconnect(now)
        done = disconnected and (is_sender or len(received) >= expect)
        sock.sendall(struct.pack("B", 1 if done else 0) + tx.tobytes())
        peer = recv_exact(sock, 1 + 4 * BLOCK)
        peer_done = peer[0] == 1
        rx = array("f")
        rx.frombytes(peer[1:])
        if done and peer_done:
            break
    else:
        say(role, f"gave up after {MAX_TICKS} ticks")
        link.close()
        return 2
    link.close()
    if not is_sender:
        print(json.dumps({"pid": os.getpid(), "received_hex": bytes(received).hex()}), flush=True)
    return 0


def run_parent():
    a, b = socket.socketpair()
    script = str(Path(__file__).resolve())
    procs = []
    for role, sock in (("listen", b), ("send", a)):
        fd = sock.fileno()
        procs.append(
            subprocess.Popen(
                [sys.executable, script, "--role", role, "--fd", str(fd), "--expect", str(len(PAYLOAD))],
                pass_fds=[fd],
                env=os.environ.copy(),
            )
        )
    a.close()
    b.close()
    rc = 0
    for p in procs:
        rc = max(rc, p.wait())
    return rc


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--role", choices=["send", "listen"])
    ap.add_argument("--fd", type=int)
    ap.add_argument("--expect", type=int, default=len(PAYLOAD))
    args = ap.parse_args()
    if args.role is None:
        return run_parent()
    if args.fd is None:
        ap.error("--fd is required with --role")
    return run_child("sender" if args.role == "send" else "listener", args.fd, args.expect)


if __name__ == "__main__":
    sys.exit(main())
