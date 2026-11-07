#!/usr/bin/env python3
"""Regenerate fixtures using a disposable pinned Direwolf source checkout.
No Direwolf implementation is copied into coppa. Python renderer is test-only.
"""
import argparse, hashlib, json, math, pathlib, subprocess, wave, struct

def render(block, output):
    bits = [((b >> i) & 1) for b in b'\x7e'*36 + block + b'\x7e'*2 for i in range(8)]
    phase, state = 0.0, True
    samples = []
    for bit in bits:
        if not bit: state = not state
        step = math.tau * (1200 if state else 2200) / 48000
        for _ in range(40):
            samples.append(round(16000 * math.sin(phase)))
            phase = (phase + step) % math.tau
    with wave.open(str(output), 'wb') as f:
        f.setparams((1,2,48000,0,'NONE','not compressed'))
        f.writeframes(struct.pack('<'+'h'*len(samples),*samples))

def main():
    a = argparse.ArgumentParser()
    a.add_argument('source',type=pathlib.Path)
    a.add_argument('output',type=pathlib.Path)
    args = a.parse_args()
    root, out = args.source.resolve(), args.output.resolve()
    assert subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip() == 'de293a1f2526ec6639fe31fa411bd4f2319ecdf4'
    # Instrument the reference transmitter just before serializing. Preserve
    # reference stuffing, FCS, parity, profile selection and audio generation.
    p = root/'src/fx25_send.c'
    s = p.read_text()
    anchor = '\n#if FXTEST\n\t// Standalone text application.'
    dump = '''
    { char path[1024]; snprintf(path,sizeof(path),"%s/tag%02d.bin",getenv("COPPA_FIXTURES"),ctag_num);
      FILE *fixture = fopen(path,"wb"); assert(fixture);
      fwrite(data,1,k_data_radio,fixture); fwrite(check,1,NROOTS,fixture); fclose(fixture); }
'''
    if 'COPPA_FIXTURES' not in s: p.write_text(s.replace(anchor,dump+anchor))
    subprocess.run(['cmake','--build',str(root/'build'),'-j4'],check=True,stdout=subprocess.DEVNULL)
    import os
    env = dict(os.environ, COPPA_FIXTURES=str(out))
    out.mkdir(parents=True,exist_ok=True)
    packet = out/'packets.txt'
    packet.write_text('N0CALL>APRS:????~~~~\n')
    for tag in range(1,12):
        subprocess.run([str(root/'build/src/gen_packets'),'-B','1200','-r','48000','-X',str(100+tag),'-o',str(out/'scratch.wav'),str(packet)],env=env,check=True,stdout=subprocess.DEVNULL)
    packet.write_text('N0CALL>APRS:coppa COP-114 FX25 fixture\n')
    subprocess.run([str(root/'build/src/gen_packets'),'-B','1200','-r','48000','-X','32','-o',str(out/'direwolf-clean.wav'),str(packet)],env=env,check=True,stdout=subprocess.DEVNULL)
    # The last call overwrote tag07; retain separate audio block, regenerate it.
    (out/'audio-block.bin').write_bytes((out/'tag07.bin').read_bytes())
    packet.write_text('N0CALL>APRS:????~~~~\n')
    subprocess.run([str(root/'build/src/gen_packets'),'-B','1200','-r','48000','-X','107','-o',str(out/'scratch.wav'),str(packet)],env=env,check=True,stdout=subprocess.DEVNULL)
    tags = [0xB74DB7DF8A532F3E,0x26FF60A600CC8FDE,0xC7DC0508F3D9B09E,0x8F056EB4369660EE,0x6E260B1AC5835FAE,0xFF94DC634F1CFF4E,0x1EB7B9CDBC09C00E,0xDBF869BD2DBB1776,0x3ADB0C13DEAE2836,0xAB69DB6A543188D6,0x4A4ABEC4A724B796]
    block = bytearray((out/'audio-block.bin').read_bytes())
    for i in range(8,16): block[i] ^= 255
    (out/'audio-damaged.bin').write_bytes(block)
    render(tags[6].to_bytes(8,'little')+block,out/'direwolf-damaged.wav')
    (out/'scratch.wav').unlink()
    packet.unlink()
    manifest = {'reference':'Direwolf 1.7 de293a1f2526ec6639fe31fa411bd4f2319ecdf4', 'license':'Generated test data; no Direwolf source distributed. Direwolf is GPL-2.0-or-later.', 'recipe':'tools/fx25-interop/generate.py SOURCE OUTPUT', 'vectors':'N0CALL>APRS:????~~~~; binary transmitted data then parity; tags 1 through 11', 'audio':'N0CALL>APRS:coppa COP-114 FX25 fixture; tag 7; clean WAV untouched gen_packets output', 'damage':'data byte offsets 8..15 XOR ff; independently re-rendered in Python, not RF', 'sha256':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name != 'manifest.json'}}
    (out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
if __name__ == '__main__': main()
