#!/usr/bin/env python3
"""Half-duplex audio cable between two coppad processes (COP-46).

Each coppad plays into, and captures from, an ALSA `file` PCM on the `null`
device (see asoundrc.in). The PCMs point at four FIFOs in DIR:

    station A plays -> ab_in  --relay-->  ab_out -> station B captures
    station B plays -> ba_in  --relay-->  ba_out -> station A captures

The relay copies each pair at real time (48 kHz mono f32). Three details from
the COP-98 loop are each needed for gap-free audio: open every FIFO O_RDWR so
no open() blocks, shrink the playback FIFOs with F_SETPIPE_SZ so a player
cannot burst ahead and underrun, and forward whole ALSA periods only (the file
plugin pads a short capture read with stale data).

On top of that it models a half-duplex radio: one monitor thread per station
holds a command-port connection open, reads every line, and tracks the PTT
state in either host vocabulary: `PTT ON`/`PTT OFF` (the VARA-style ports)
and `PTT TRUE`/`PTT FALSE` (the ARDOP ports, COP-97). While a station is
keyed, the relay feeds it silence.

Usage: relay.py DIR A_CMD_PORT B_CMD_PORT
Python 3 standard library only.
"""

import fcntl
import os
import socket
import sys
import threading
import time
import traceback

RATE = 48000 * 4  # bytes per second: f32 mono
CHUNK = 240 * 4 * 4  # four whole 240-frame ALSA periods
F_SETPIPE_SZ = 1031


def monitor(port, keyed):
    """Track one station's PTT from its command port, reconnecting.

    Both surfaces report PTT: VARA-style as ON/OFF, ARDOP as TRUE/FALSE.
    """
    while True:
        try:
            with socket.create_connection(("127.0.0.1", port)) as sock:
                pending = b""
                while True:
                    data = sock.recv(4096)
                    if not data:
                        break
                    pending += data
                    *lines, pending = pending.split(b"\r")
                    for line in lines:
                        line = line.strip()
                        if line in (b"PTT ON", b"PTT TRUE"):
                            keyed.set()
                        elif line in (b"PTT OFF", b"PTT FALSE"):
                            keyed.clear()
        except OSError:
            pass
        keyed.clear()
        time.sleep(0.2)


def pump(fds, src, dst, listener_keyed):
    """Copy src to dst at real time; silence while the listener is keyed."""
    t0, sent, pend = time.monotonic(), 0, bytearray()
    silence = bytes(CHUNK)
    while True:
        pend += os.read(fds[src], CHUNK)
        while len(pend) >= CHUNK:
            block = bytes(pend[:CHUNK])
            del pend[:CHUNK]
            os.write(fds[dst], silence if listener_keyed.is_set() else block)
            sent += CHUNK
            ahead = sent / RATE - (time.monotonic() - t0)
            if ahead > 0:
                time.sleep(ahead)
            elif ahead < -0.5:
                t0, sent = time.monotonic(), 0


def fatal_on_error(target):
    """Run target; if it raises, exit the whole relay so the cable visibly
    breaks (coppad logs an output failure) instead of one direction dying
    silently while the process stays up."""

    def run(*args):
        try:
            target(*args)
        except BaseException:
            traceback.print_exc()
            sys.stderr.flush()
            os._exit(1)

    return run


def main():
    if len(sys.argv) != 4:
        sys.exit("usage: relay.py DIR A_CMD_PORT B_CMD_PORT")
    directory, a_port, b_port = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    names = ("ab_in", "ab_out", "ba_in", "ba_out")
    for name in names:
        path = os.path.join(directory, name)
        if os.path.exists(path):
            os.unlink(path)
        os.mkfifo(path)
    fds = {n: os.open(os.path.join(directory, n), os.O_RDWR) for n in names}
    for name in ("ab_in", "ba_in"):
        fcntl.fcntl(fds[name], F_SETPIPE_SZ, 4096)

    a_keyed, b_keyed = threading.Event(), threading.Event()
    threads = [
        threading.Thread(target=fatal_on_error(monitor), args=(a_port, a_keyed), daemon=True),
        threading.Thread(target=fatal_on_error(monitor), args=(b_port, b_keyed), daemon=True),
        # A -> B: B is deaf while B transmits; B -> A likewise for A.
        threading.Thread(target=fatal_on_error(pump), args=(fds, "ab_in", "ab_out", b_keyed), daemon=True),
        threading.Thread(target=fatal_on_error(pump), args=(fds, "ba_in", "ba_out", a_keyed), daemon=True),
    ]
    for t in threads:
        t.start()
    while True:
        time.sleep(3600)


if __name__ == "__main__":
    main()
