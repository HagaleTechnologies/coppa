#!/usr/bin/env python3
"""Extract one file from an ISO9660 image, preferring Joliet names. Stdlib only.

COP-112: tools/tnc-bench/run.sh uses this to pull WA8LMF TNC Test CD track 2
out of TNC_Test_2.iso, then checks the result against a pinned SHA-256.

usage: iso_extract.py IMAGE PATH/IN/ISO OUTFILE
"""
import struct
import sys

SECTOR = 2048
USAGE = "usage: iso_extract.py IMAGE PATH/IN/ISO OUTFILE"


def read_sector(f, lba, count=1):
    f.seek(lba * SECTOR)
    return f.read(count * SECTOR)


def find_root(f):
    """Return (root_lba, root_len, joliet) from the best volume descriptor."""
    primary = None
    joliet = None
    lba = 16
    while True:
        vd = read_sector(f, lba)
        if vd[1:6] != b"CD001":
            raise SystemExit("not an ISO9660 image")
        vtype = vd[0]
        root = vd[156:156 + 34]
        ext, size = struct.unpack_from("<I", root, 2)[0], struct.unpack_from("<I", root, 10)[0]
        if vtype == 1 and primary is None:
            primary = (ext, size, False)
        elif vtype == 2 and vd[88:91] in (b"%/@", b"%/C", b"%/E"):
            joliet = (ext, size, True)
        elif vtype == 255:
            break
        lba += 1
    return joliet or primary


def entries(f, lba, size, joliet):
    data = read_sector(f, lba, (size + SECTOR - 1) // SECTOR)[:size]
    i = 0
    while i < len(data):
        n = data[i]
        if n == 0:  # records never straddle a sector; skip to the next one
            i = (i // SECTOR + 1) * SECTOR
            continue
        rec = data[i:i + n]
        ext = struct.unpack_from("<I", rec, 2)[0]
        length = struct.unpack_from("<I", rec, 10)[0]
        is_dir = bool(rec[25] & 0x02)
        raw = rec[33:33 + rec[32]]
        if raw not in (b"\x00", b"\x01"):
            name = raw.decode("utf-16-be") if joliet else raw.decode("ascii")
            name = name.split(";")[0]
            yield name, ext, length, is_dir
        i += n


def main():
    if len(sys.argv) < 4:
        print(USAGE, file=sys.stderr)
        sys.exit(2)
    image, want, out = sys.argv[1], sys.argv[2], sys.argv[3]
    with open(image, "rb") as f:
        lba, size, joliet = find_root(f)
        parts = want.strip("/").split("/")
        for depth, part in enumerate(parts):
            last = depth == len(parts) - 1
            for name, ext, length, is_dir in entries(f, lba, size, joliet):
                if name.casefold() == part.casefold() and is_dir != last:
                    lba, size = ext, length
                    break
            else:
                raise SystemExit(f"{want}: not found in {image}")
        f.seek(lba * SECTOR)
        remaining = size
        with open(out, "wb") as o:
            while remaining:
                chunk = f.read(min(remaining, 1 << 20))
                if not chunk:
                    raise SystemExit("image truncated")
                o.write(chunk)
                remaining -= len(chunk)


if __name__ == "__main__":
    main()
