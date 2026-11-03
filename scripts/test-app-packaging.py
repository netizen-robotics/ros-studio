"""Offline packaging contract checks; fake binaries do not prove native execution."""
import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('windows_packager', ROOT / 'scripts/package-apps-windows.py')
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)
ICONS = ROOT / 'crates/ui-foundation/assets/app-icons'


class PackagingContractTests(unittest.TestCase):
    def test_icon_formats_contain_bounded_native_size_representations(self):
        for app in ('launcher', 'visualizer', 'tools'):
            with self.subTest(app=app):
                data = (ICONS / f'{app}.ico').read_bytes()
                self.assertEqual(struct.unpack_from('<HHH', data), (0, 1, 7))
                sizes = []
                for index in range(7):
                    w, h, _, _, planes, depth, length, offset = struct.unpack_from('<BBBBHHII', data, 6 + index * 16)
                    self.assertEqual((planes, depth), (1, 32))
                    self.assertLessEqual(offset + length, len(data))
                    png = data[offset:offset + length]
                    self.assertEqual(png[:8], b'\x89PNG\r\n\x1a\n')
                    size = w or 256
                    self.assertEqual(struct.unpack_from('>II', png, 16), (size, h or 256))
                    sizes.append(size)
                self.assertEqual(sizes, [16, 24, 32, 48, 64, 128, 256])
                icns = (ICONS / f'{app}.icns').read_bytes()
                self.assertEqual(icns[:4], b'icns')
                self.assertEqual(struct.unpack_from('>I', icns, 4)[0], len(icns))
                offset, sizes = 8, []
                while offset < len(icns):
                    tag, length = struct.unpack_from('>4sI', icns, offset)
                    self.assertIn(tag, (b'ic07', b'ic08', b'ic09', b'ic10'))
                    self.assertLessEqual(offset + length, len(icns))
                    png = icns[offset + 8:offset + length]
                    self.assertEqual(png[:8], b'\x89PNG\r\n\x1a\n')
                    sizes.append(struct.unpack_from('>II', png, 16))
                    offset += length
                self.assertEqual(sizes, [(128, 128), (256, 256), (512, 512), (1024, 1024)])

    def fixture(self, folder):
        root = Path(folder)
        binaries, runtime = root / 'binaries', root / 'Library'
        binaries.mkdir()
        (runtime / 'bin').mkdir(parents=True)
        (runtime / 'share/ament_index').mkdir(parents=True)
        (runtime / 'bin/fixture.dll').write_bytes(b'FAKE TEST DLL')
        for app in ('bootstrap', 'launcher', 'backend', 'network-helper', 'tools', 'visualizer'):
            (binaries / f'studio-{app}.exe').write_bytes(b'FAKE TEST EXECUTABLE ' + app.encode())
        notices = root / 'notices.txt'
        notices.write_text('Synthetic dependency notices; not a native package qualification.')
        return argparse.Namespace(runtime=runtime, binaries=binaries, notices=notices,
                                  destination=root / 'output', release_base_url=None, signtool=None)

    def assemble(self, args):
        with patch.object(packager.platform, 'system', return_value='Windows'), \
             patch.object(packager.platform, 'machine', return_value='AMD64'), \
             contextlib.redirect_stdout(io.StringIO()):
            packager.assemble(args)

    def test_windows_archives_retain_project_license_and_hashed_inventory(self):
        with tempfile.TemporaryDirectory() as folder:
            args = self.fixture(folder)
            self.assemble(args)
            setup = next(args.destination.glob('ros-studio-setup-0.1.0-*.zip'))
            with zipfile.ZipFile(setup) as archive:
                self.assertEqual(archive.read('LICENSE'), (ROOT / 'LICENSE').read_bytes())
                self.assertIn('studio-launcher.exe', archive.namelist())
            for app in ('tools', 'visualizer'):
                manifest = json.loads((args.destination / app / 'manifest.json').read_text())
                self.assertEqual(manifest['version'], '0.1.0')
                self.assertEqual(manifest['files']['LICENSE'], packager.digest(ROOT / 'LICENSE'))
                with tarfile.open(next(args.destination.glob(f'studio-{app}-*.tar.gz'))) as archive:
                    self.assertEqual(archive.extractfile(f'{app}/LICENSE').read(), (ROOT / 'LICENSE').read_bytes())
            with self.assertRaisesRegex(ValueError, 'previous evidence'):
                self.assemble(args)

    def test_windows_schema2_catalog_targets_the_first_launcher_version(self):
        with tempfile.TemporaryDirectory() as folder:
            args = self.fixture(folder)
            args.release_base_url = 'https://github.com/example/fixture/releases/download/v0.1.0'
            args.signtool = Path('fixture-signtool')
            with patch.object(packager.subprocess, 'check_output', return_value=json.dumps({'publisher_sha256': 'fixture', 'catalog_key': 'fixture'})), \
                 patch.object(packager.subprocess, 'run') as verification:
                self.assemble(args)
            self.assertEqual(verification.call_count, 6)
            catalog = json.loads((args.destination / 'catalog-unsigned.json').read_text())
            self.assertEqual(catalog['schema'], 2)
            self.assertEqual(len(catalog['releases']), 2)
            for release in catalog['releases']:
                self.assertEqual(release['minimum_launcher'], '0.1.0')
                self.assertEqual(release['version'], '0.1.0')
                self.assertEqual(len(release['windows']['manifest_sha256']), 64)


if __name__ == '__main__':
    unittest.main()
