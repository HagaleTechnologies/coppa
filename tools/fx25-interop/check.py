#!/usr/bin/env python3
"""Check actual atest output, including its FX.25 diagnostic and hex dump."""
import pathlib, re, sys
EXPECTED = bytes.fromhex('82 a0 a4 a6 40 40 e0 9c 60 86 82 98 98 e1 03 f0') + b'coppa COP-114 FX25 fixture\n'
def check(raw):
    text = re.sub(r'\x1b\[[0-9;]*m', '', raw.decode('ascii', errors='replace'))
    if 'Matched correlation tag 0x07 with 0 bit errors' not in text:
        raise ValueError('missing expected tag 7 diagnostic')
    if 'FEC complete with no errors.' not in text:
        raise ValueError('missing zero-error FX.25 FEC diagnostic')
    frames = re.findall(r'^\[0\] (.*)$',text,re.M)
    if frames != ['N0CALL>APRS:coppa COP-114 FX25 fixture<0x0a>']:
        raise ValueError(f'unexpected decoded frames: {frames!r}')
    decoded = bytearray()
    for line in text.splitlines():
        m = re.match(r'^  [0-9a-f]{3}:  ((?:[0-9a-f]{2} ?){1,16})',line)
        if m: decoded.extend(bytes.fromhex(m[1]))
    if decoded != EXPECTED:
        raise ValueError(f'wrong AX.25 bytes: {decoded.hex()}')
    print('tag 7; FX.25 FEC complete with no errors; exact 43-byte AX.25 payload verified')
if __name__ == '__main__': check(pathlib.Path(sys.argv[1]).read_bytes())
