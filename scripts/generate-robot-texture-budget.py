"""Generate intentional synthetic raster-budget inputs under ignored evidence."""
import argparse
from pathlib import Path
import struct
import sys
import zlib
ROOT = Path(__file__).resolve().parents[1]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        parser.error('Use project Pixi Python')
    out = args.output.resolve()
    if not out.is_relative_to(ROOT / 'artifacts/feasibility/evidence') or out.exists():
        parser.error('Choose a new directory under ignored artifacts/feasibility/evidence')
    out.mkdir(parents=True)
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)
    for name, color in [('red', bytes([255, 0, 0, 255])), ('green', bytes([0, 255, 0, 255])), ('blue', bytes([0, 0, 255, 128]))]:
        scan = (b'\0' + color * 2048) * 2048
        data = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2048, 2048, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(scan, 1)) + chunk(b'IEND', b'')
        (out / (name + '.png')).write_bytes(data)
    def visual(name):
        return f'<visual><geometry><box size="1 1 1"/></geometry><material><texture filename="{name}.png"/></material></visual>'
    (out / 'maximum.urdf').write_text('<robot name="texture_budget"><link name="base">' + visual('red') + visual('green') + '</link></robot>')
    (out / 'over.urdf').write_text('<robot name="texture_budget"><link name="base">' + ''.join(visual(c) for c in ['red','green','blue']) + '</link></robot>')
    print(out)

if __name__ == '__main__':
    main()
