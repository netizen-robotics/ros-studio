"""Offline regression checks for ignored evidence indexing in public checkouts."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('snapshot_evidence', ROOT / 'scripts/snapshot-evidence.py')
indexer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(indexer)


class EvidenceIndexTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        self.raw = self.root / 'artifacts/feasibility'
        self.raw.mkdir(parents=True)
        self.target = self.raw / 'RESULTS.json'
        self.root_patch = patch.object(indexer, 'ROOT', self.root)
        self.raw_patch = patch.object(indexer, 'RAW', self.raw)
        self.root_patch.start()
        self.raw_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.addCleanup(self.raw_patch.stop)

    def run_main(self, *args):
        output = io.StringIO()
        with patch.object(sys, 'argv', ['snapshot-evidence.py', *args]), contextlib.redirect_stdout(output):
            indexer.main()
        return output.getvalue()

    def write_report(self, outcome='passed'):
        (self.raw / 'fixture.json').write_text(json.dumps({
            'os': 'fixture OS', 'cpu': 'fixture CPU', 'distro': 'jazzy',
            'middleware_pair': 'offline', 'outcome': outcome,
        }))

    def test_fresh_checkout_check_does_not_claim_evidence_or_create_index(self):
        self.raw.rmdir()
        self.assertIn('not tested', self.run_main('--check'))
        self.assertIn('nothing to index', self.run_main())
        self.assertFalse(self.target.exists())
        self.assertFalse((self.root / 'docs').exists())

    def test_repeated_snapshot_excludes_itself_and_is_stable(self):
        self.write_report()
        self.run_main()
        first = self.target.read_bytes()
        self.run_main()
        self.assertEqual(self.target.read_bytes(), first)
        self.assertEqual([row['report'] for row in indexer.summary()['reports']], ['fixture.json'])
        self.assertIn('index passed', self.run_main('--check'))

    def test_preserves_unavailable_historical_records(self):
        historical = {'report': 'unavailable.json', 'artifact': 'artifacts/feasibility/unavailable.json',
                      'sha256': 'historical', 'records': 1, 'outcomes': {'failed': 1}, 'contexts': []}
        self.target.write_text(json.dumps({'schema_version': 1, 'reports': [historical]}))
        self.write_report()
        self.run_main()
        self.assertIn(historical, json.loads(self.target.read_text())['reports'])

    def test_modified_report_fails_check_until_snapshot_is_refreshed(self):
        self.write_report()
        self.run_main()
        self.write_report('failed')
        with self.assertRaisesRegex(SystemExit, 'stale'):
            self.run_main('--check')
        self.run_main()
        self.assertIn('index passed', self.run_main('--check'))

    def test_invalid_outcome_does_not_overwrite_existing_index(self):
        self.write_report()
        self.run_main()
        before = self.target.read_bytes()
        self.write_report('unknown')
        with self.assertRaisesRegex(ValueError, 'Invalid outcome'):
            self.run_main()
        self.assertEqual(before, self.target.read_bytes())

    def test_invalid_existing_index_does_not_erase_history(self):
        self.target.write_text('{"schema_version": 2, "reports": []}')
        before = self.target.read_bytes()
        self.write_report()
        with self.assertRaisesRegex(ValueError, 'refusing to replace history'):
            self.run_main()
        self.assertEqual(before, self.target.read_bytes())


if __name__ == '__main__':
    unittest.main()
