"""Offline Windows setup layout checks; native installer checks run on Windows."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('setup', Path(__file__).with_name('package-suite-setup.py'))
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


class SetupLayoutTests(unittest.TestCase):
    def test_offline_suite_has_one_runtime_and_no_legacy_helper(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            runtime, binaries, layout = base / 'Library', base / 'bin', base / 'layout'
            (runtime / 'bin').mkdir(parents=True)
            (runtime / 'bin/fixture.dll').write_bytes(b'fixture runtime')
            (runtime / 'share/ament_index').mkdir(parents=True)
            binaries.mkdir()
            for role in (*setup.ROLES, 'network-helper', 'bootstrap'):
                (binaries / f'studio-{role}.exe').write_bytes(b'fixture executable')
            files = setup.stage(runtime, binaries, layout)
            self.assertTrue(all(f'studio-{r}.exe' in files for r in setup.ROLES))
            self.assertIn('runtime/bin/fixture.dll', files)
            self.assertIn('runtime/manifest.json', files)
            self.assertNotIn('studio-network-helper.exe', files)
            self.assertNotIn('studio-bootstrap.exe', files)
            self.assertTrue((layout / 'notices/ros-interfaces/NOTICE.txt').is_file())

    def test_missing_backend_fails_before_creating_layout(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            (base / 'runtime/bin').mkdir(parents=True)
            (base / 'runtime/share/ament_index').mkdir(parents=True)
            with self.assertRaisesRegex(ValueError, 'Missing studio-launcher'):
                setup.stage(base / 'runtime', base / 'bin', base / 'out')
            self.assertFalse((base / 'out').exists())


if __name__ == '__main__':
    unittest.main()
