"""Offline suite packaging regressions; mock SDK output is not MSIX validation."""
import argparse
import contextlib
import importlib.util
import io
import hashlib
import json
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('suite_windows', ROOT / 'scripts/package-suite-windows.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SuitePackagingTests(unittest.TestCase):
    def fixture(self, folder):
        base = Path(folder)
        binaries, runtime = base / 'bin', base / 'runtime'
        binaries.mkdir()
        for role in (*module.ROLES, 'network-helper', 'bootstrap'):
            (binaries / f'studio-{role}.exe').write_bytes(b'fixture executable')
        (runtime / 'bin').mkdir(parents=True)
        (runtime / 'bin/fixture.dll').write_bytes(b'runtime')
        (runtime / 'share/ament_index').mkdir(parents=True)
        notices = base / 'notices.txt'
        notices.write_text('Synthetic notices for offline test only')
        return argparse.Namespace(runtime=runtime, binaries=binaries, destination=base / 'out', notices=notices,
                                  makeappx=Path('fixture-makeappx'), identity_name='Example.ROSStudio', publisher='CN=example & test', publisher_display_name='Example')

    def test_single_package_excludes_legacy_installer_and_elevation(self):
        with tempfile.TemporaryDirectory() as folder:
            args = self.fixture(folder)
            def sdk(command, **kwargs):
                Path(command[command.index('/p') + 1]).write_bytes(b'mock MSIX: not native validation')
            with patch.object(module.platform, 'system', return_value='Windows'), patch.object(module.platform, 'machine', return_value='AMD64'), patch.object(module.subprocess, 'check_output', return_value=json.dumps({'Comments': 'ROS Studio distribution: microsoft-store', 'FileVersion':'0.1.0'})) as config, patch.object(module.subprocess, 'run', side_effect=sdk), contextlib.redirect_stdout(io.StringIO()):
                module.assemble(args)
            self.assertEqual(config.call_count, 3)
            layout = args.destination / 'layout'
            self.assertFalse((layout / 'studio-network-helper.exe').exists())
            self.assertFalse((layout / 'studio-bootstrap.exe').exists())
            self.assertTrue(all((layout / f'studio-{r}.exe').exists() for r in module.ROLES))
            manifest = ET.parse(layout / 'AppxManifest.xml').getroot()
            self.assertEqual(manifest.find(f'{{{module.NS}}}Identity').attrib['Publisher'], args.publisher)
            self.assertEqual(manifest.find(f'{{{module.NS}}}Identity').attrib['Version'], '0.1.0.0')
            self.assertEqual(len(list((layout / 'runtime/bin').iterdir())), 1)
            caps = manifest.find(f'{{{module.NS}}}Capabilities')
            self.assertNotIn('allowElevation', [c.attrib['Name'] for c in caps])
            self.assertEqual(len(manifest.find(f'{{{module.NS}}}Applications')), 1)

    def test_rejects_frontend_without_store_build_before_writing(self):
        with tempfile.TemporaryDirectory() as folder:
            args = self.fixture(folder)
            with patch.object(module.platform, 'system', return_value='Windows'), patch.object(module.platform, 'machine', return_value='AMD64'), patch.object(module.subprocess, 'check_output', return_value=json.dumps({'Comments': 'ROS Studio distribution: bundled', 'FileVersion':'0.1.0'})):
                with self.assertRaisesRegex(ValueError, 'store-distribution'):
                    module.assemble(args)
            self.assertFalse(args.destination.exists())

    def test_nav2_interfaces_and_notices_survive_suite_layout(self):
        with tempfile.TemporaryDirectory() as folder:
            args = self.fixture(folder)
            name = 'nav2_msgs__rosidl_typesupport_introspection_c.dll'
            (args.runtime/'bin'/name).write_bytes(b'fixture Nav2 native library')
            action = args.runtime/'share/nav2_msgs/action/NavigateToPose.action'
            action.parent.mkdir(parents=True)
            action.write_text('fixture action definition')
            def sdk(command, **_kwargs):
                Path(command[command.index('/p')+1]).write_bytes(b'fixture MSIX')
            with patch.object(module.platform,'system',return_value='Windows'), patch.object(module.platform,'machine',return_value='AMD64'), patch.object(module.subprocess,'check_output',return_value=json.dumps({'Comments':'ROS Studio distribution: microsoft-store'})), patch.object(module.subprocess,'run',side_effect=sdk), contextlib.redirect_stdout(io.StringIO()):
                module.assemble(args)
            self.assertEqual((args.destination/'layout/runtime/bin'/name).read_bytes(), b'fixture Nav2 native library')
            self.assertEqual((args.destination/'layout/runtime/share/nav2_msgs/action/NavigateToPose.action').read_text(), 'fixture action definition')
            self.assertTrue((args.destination/'layout/ros-interface-notices/Apache-2.0.txt').is_file())

    def test_macos_trim_keeps_dynamically_loaded_nav2_and_geographic_support(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder); source = base/'source'; destination = base/'trimmed'
            (source/'lib').mkdir(parents=True); (source/'bin').mkdir()
            (source/'share/ament_index').mkdir(parents=True)
            (source/'dependency-metadata').mkdir()
            (source/'notices').mkdir(); (source/'notices/fixture.txt').write_text('fixture')
            (source/'manifest.json').write_text(json.dumps({'distro':'jazzy'}))
            names = ['studio-backend','studio-desktop','librmw_fastrtps_cpp.dylib','librmw_cyclonedds_cpp.dylib','librcl_logging_spdlog.dylib','libnav2_msgs__rosidl_typesupport_introspection_c.dylib','libnav2_msgs__rosidl_typesupport_fastrtps_c.dylib','libnav2_msgs__rosidl_typesupport_c.dylib','libgeographic_msgs__rosidl_typesupport_introspection_c.dylib']
            records=[]
            for name in names:
                path = source/('bin' if name.startswith('studio-') else 'lib')/name
                path.write_bytes(('fixture '+name).encode())
                records.append(dict(file=str(path.relative_to(source)),sha256=hashlib.sha256(path.read_bytes()).hexdigest(),dependencies=[]))
            inventory=base/'inventory.json'; inventory.write_text(json.dumps(records))
            subprocess.run([sys.executable,str(ROOT/'scripts/trim-macos-bundle.py'),'--source',str(source),'--inventory',str(inventory),'--destination',str(destination)],check=True,capture_output=True)
            for name in names[5:]:
                self.assertEqual((destination/'lib'/name).read_bytes(),(source/'lib'/name).read_bytes())
            self.assertTrue((destination/'notices/fixture.txt').is_file())


if __name__ == '__main__':
    unittest.main()
