#!/usr/bin/env python3
"""Generate independent fixtures using an external Direwolf 1.7 source tree.

Usage: python3 testdata/il2p/generate.py /path/to/direwolf-1.7
Only the external source tree is patched. No Direwolf implementation is copied
into Coppa. C glue passes raw AX.25 to Direwolf's existing public test APIs.
"""
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent
source = pathlib.Path(sys.argv[1]).resolve()
p = source / 'src/gen_packets.c'
original = p.read_text()
glue = r'''
          unsigned char raw[AX25_MAX_PACKET_LEN];
          int raw_len = 0;
          unsigned int octet;
          for (char *hex = str; hex[0] && hex[1] && raw_len < 2048; hex += 2) {
              if (sscanf(hex, "%2x", &octet) != 1) break;
              raw[raw_len++] = octet;
          }
          alevel_t level = {0};
          pp = ax25_from_frame(raw, raw_len, level);
          unsigned char encoded[2048];
          int length = il2p_encode_frame(pp, modem.achan[0].il2p_max_fec, encoded);
          printf("WIRE ");
          for (int k=0; k<length; k++) printf("%02x", encoded[k]);
          printf("\n");
          packet_t decoded = il2p_decode_frame(encoded);
          unsigned char expected[AX25_MAX_PACKET_LEN];
          int expected_len = ax25_pack(decoded, expected);
          printf("FRAME ");
          for (int k=0; k<expected_len; k++) printf("%02x", expected[k]);
          printf("\n");
          ax25_delete(decoded);
'''
# This is test glue around the external encoder, not a port of its algorithm.
assert original.count('pp = ax25_from_text (str, 1);') == 2
p.write_text(original.replace('pp = ax25_from_text (str, 1);', glue))
build = source / 'coppa-fixture-build'
try:
    subprocess.run(['cmake', '-S', str(source), '-B', str(build)], check=True, stdout=subprocess.DEVNULL)
    subprocess.run(['cmake', '--build', str(build), '--target', 'gen_packets', '-j', '4'], check=True, stdout=subprocess.DEVNULL)
    def address(call, ssid):
        return bytes(c << 1 for c in call.ljust(6).encode()) + bytes([ssid])
    base = address('APRS', 0xe0) + address('N0CALL', 0x61)
    cases = {
        'ui-max': (base + b'\x03\xf0COP-115\x00\xc0\xdb\x91\x27', 1, False),
        'ui-auto-inverted': (base + b'\x13\xf0automatic FEC', 0, True),
        'i': (base + b'\xb6\xccI frame', 1, False),
        's': (base + b'\xb9', 1, False),
        'u': (base + b'\x63', 1, False),
        'transparent': (base[:13] + b'\x60' + address('WIDE1', 0x63) + b'\x03\xf0via digipeater\xc0\xdb', 1, False),
        'maximum': (base + b'\x03\xf0' + bytes(i % 256 for i in range(1023)), 1, False),
    }
    def generate(name, frame, fec, inverted, wav):
        # gen_packets' input buffer is 400 bytes in the release. Increase only
        # that test input buffer to carry full raw hex frames.
        cmd = [str(build/'src/gen_packets'), '-i' if inverted else '-I', str(fec), '-r', '48000', '-o', str(wav), '-']
        result = subprocess.run(cmd, input=frame.hex()+'\n', text=True, capture_output=True, check=True)
        import re
        lines = re.sub(r'\x1b\[[0-9;]*m', '', result.stdout).splitlines()
        wire = re.search(r'WIRE ([0-9a-f]+)', result.stdout)[1]
        expected = next(line.removeprefix('FRAME ') for line in lines if line.startswith('FRAME '))
        if not name.startswith('vector-'):
            (root/(name+'.wire')).write_bytes(bytes.fromhex(wire))
            (root/(name+'.ax25')).write_bytes(bytes.fromhex(expected))
        else:
            vectors.append(f'{name} {wire}')
        return bytes.fromhex(expected)
    # Main has a 400 byte line buffer. Rebuild with enough space for max frames.
    patched = p.read_text().replace('char str[400]', 'char str[8192]').replace('char str[256]', 'char str[8192]')
    p.write_text(patched)
    subprocess.run(['cmake', '--build', str(build), '--target', 'gen_packets', '-j', '4'], check=True, stdout=subprocess.DEVNULL)
    for name, (frame, fec, inverted) in cases.items():
        assert generate(name, frame, fec, inverted, root/(name+'.wav')) == frame, name
    vectors = []
    # Byte vectors cover every payload-plan boundary for both FEC choices.
    for n in [0,1,61,62,123,124,185,186,239,240,247,248,477,478,1023]:
        for fec in [0,1]:
            name = f'vector-{n}-{fec}'
            frame = base + b'\x03\xf0' + bytes(i % 256 for i in range(n))
            assert generate(name, frame, fec, False, build/'vector.wav') == frame
    (root/'vectors.txt').write_text('\n'.join(vectors)+'\n')
finally:
    p.write_text(original)
