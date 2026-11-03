"""Render original vector app icons for native windows, macOS and Windows.

Development only: needs rsvg-convert. Encodes ICO and ICNS without platform tools.
Run with scripts/dev .pixi/envs/default/bin/python. Runtime needs neither tool.
"""
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'crates/ui-foundation/assets/app-icons'


def render(source, output, size):
    subprocess.run(['rsvg-convert', '--width', str(size), '--height', str(size),
                    '--output', str(output), str(source)], check=True)


def main():
    if not shutil.which('rsvg-convert'):
        raise SystemExit('Install the development SVG renderer rsvg-convert first')
    (ROOT / 'artifacts').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='app-icons-', dir=ROOT / 'artifacts') as folder:
        work = Path(folder)
        for app in ('launcher', 'visualizer', 'tools'):
            source = ASSETS / f'{app}.svg'
            render(source, ASSETS / f'{app}.png', 256)
            images = []
            for size in (16, 24, 32, 48, 64, 128, 256):
                output = work / f'{app}-{size}.png'
                render(source, output, size)
                images.append((size, output.read_bytes()))
            offset = 6 + 16 * len(images)
            directory = []
            for size, data in images:
                directory.append(struct.pack('<BBBBHHII', size % 256, size % 256,
                                             0, 0, 1, 32, len(data), offset))
                offset += len(data)
            (ASSETS / f'{app}.ico').write_bytes(
                struct.pack('<HHH', 0, 1, len(images)) + b''.join(directory)
                + b''.join(data for _, data in images))
            # ICNS stores native PNG representations in big-endian typed chunks.
            chunks = []
            for size, tag in ((128, b'ic07'), (256, b'ic08'),
                              (512, b'ic09'), (1024, b'ic10')):
                output = work / f'{app}-icns-{size}.png'
                render(source, output, size)
                data = output.read_bytes()
                chunks.append(tag + struct.pack('>I', len(data) + 8) + data)
            payload = b''.join(chunks)
            (ASSETS / f'{app}.icns').write_bytes(
                b'icns' + struct.pack('>I', len(payload) + 8) + payload)
            if app == 'launcher':
                for size in (44, 50, 150):
                    render(source, ASSETS / f'suite-{size}.png', size)
            print(f'Rendered {app}')


if __name__ == '__main__':
    main()
