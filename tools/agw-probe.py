#!/usr/bin/env python3
"""Probe AGWPE control replies. This does not verify audio or RF delivery."""
import argparse
import socket
import struct


def packet(kind, body=b"", source=b"N0CALL", dest=b"APRS", pid=0xF0):
    return struct.pack("<B3xBxBx10s10sI4x", 0, ord(kind), pid, source, dest, len(body)) + body


def reply(sock, kind):
    def exact(n):
        data = bytearray()
        while len(data) < n:
            chunk = sock.recv(n - len(data))
            if not chunk:
                raise RuntimeError("EOF in AGWPE reply")
            data.extend(chunk)
        return data
    header = exact(36)
    assert header[4] == ord(kind), header
    length = struct.unpack_from("<I", header, 28)[0]
    assert length <= 1024, length
    return bytes(exact(length))


# The G port description and g baud code (byte 0) the TNC reports at each
# --baud (COP-164).
PORTS = {1200: (b"1;Port1 coppa AFSK 1200;\0", 0),
         9600: (b"1;Port1 coppa G3RUH 9600;\0", 3)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--baud", type=int, choices=sorted(PORTS), default=1200,
                        help="the --baud the TNC was started with (default 1200)")
    args = parser.parse_args()
    port_text, baud_code = PORTS[args.baud]
    with socket.create_connection((args.host, args.port), timeout=3) as sock:
        for kind, expected in [("R", struct.pack("<II", 2005, 127)),
                               ("G", port_text),
                               ("g", bytes([baud_code, 255]) + bytes(10)), ("X", b"\1")]:
            sock.sendall(packet(kind))
            got = reply(sock, kind)
            assert got == expected, f"{kind}: got {got!r}, expected {expected!r}"
        sock.sendall(packet("k") + packet("m") + packet("M", b">coppa AGW probe"))
    print("Control replies verified; UI transmission requested. Audio and RF delivery NOT tested.")


if __name__ == "__main__":
    main()
