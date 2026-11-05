"""Offline checks for runtime dependency closure; no native acceptance implied."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('deb', Path(__file__).with_name('package-suite-deb.py'))
deb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deb)


class RuntimeTests(unittest.TestCase):
    def fixture(self, folder):
        prefix, app = Path(folder) / 'prefix', Path(folder) / 'app'
        (prefix / 'lib').mkdir(parents=True)
        app.mkdir()
        for role in deb.ROLES:
            (app / f'studio-{role}').write_bytes(b'\x7fELFfixture')
        return prefix, app

    def test_closes_plugin_dependencies_and_does_not_bundle_libc(self):
        with tempfile.TemporaryDirectory() as folder:
            prefix, app = self.fixture(folder)
            for name in ('librmw_fixture.so', 'libdependency.so', 'libc.so.6', 'libunused.so'):
                (prefix / 'lib' / name).write_bytes(b'\x7fELFfixture')
            def needed(path):
                return ['libdependency.so', 'libc.so.6'] if path.name == 'librmw_fixture.so' else []
            with patch.object(deb, 'dependencies', side_effect=needed), patch.object(deb, 'run', return_value='') as run:
                names = deb.copy_runtime(prefix, app)
            self.assertEqual(names, ['libdependency.so', 'librmw_fixture.so'])
            self.assertTrue(any('$ORIGIN/runtime/lib' in call.args for call in run.call_args_list))

    def test_rejects_library_link_outside_runtime(self):
        with tempfile.TemporaryDirectory() as folder:
            prefix, app = self.fixture(folder)
            outside = Path(folder) / 'outside.so'
            outside.write_bytes(b'\x7fELFfixture')
            (prefix / 'lib/librmw_escape.so').symlink_to(outside)
            with self.assertRaisesRegex(ValueError, 'Invalid runtime library'):
                deb.copy_runtime(prefix, app)

    def test_missing_dependency_is_not_a_successful_package(self):
        with tempfile.TemporaryDirectory() as folder:
            prefix, app = self.fixture(folder)
            (prefix / 'lib/librmw_fixture.so').write_bytes(b'\x7fELFfixture')
            with patch.object(deb, 'dependencies', return_value=[]), patch.object(deb, 'run', return_value='libmissing.so => not found'):
                with self.assertRaisesRegex(ValueError, 'Unresolved'):
                    deb.copy_runtime(prefix, app)


if __name__ == '__main__':
    unittest.main()
