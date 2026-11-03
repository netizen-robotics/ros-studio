"""Pressure helper admission, coalescing and reserved control tests; no ROS."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('image_pressure', ROOT / 'scripts/test-image-session-pressure.py')
pressure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pressure)


def sample(sub, sequence, drops=0):
    return {'event': 'image_observation' if sub == 1 else 'observation', 'generation': 81,
            'subscription': sub, 'sequence': sequence, 'backend_drops': drops, 'sample': {}}


def stream(events):
    return io.BytesIO(b''.join(json.dumps(event).encode() + b'\n' for event in events))


class PressureReaderTests(unittest.TestCase):
    def test_flood_replaces_only_latest_sample_and_reserves_ordered_control(self):
        events = [sample(1, sequence, sequence // 10) for sequence in range(1, 1001)]
        events.extend([sample(2, 1), {'event': 'graph', 'topics': []},
                       {'event': 'drops', 'generation': 81, 'subscription': 1, 'backend_drops': 100},
                       {'event': 'stopped', 'generation': 81, 'subscription': 1, 'request': 3}])
        with tempfile.TemporaryDirectory() as temporary:
            reader = pressure.Reader(stream(events), Path(temporary))
            reader.thread.join(timeout=2)
            self.assertFalse(reader.thread.is_alive())
            self.assertIsNone(reader.error)
            self.assertEqual(len(reader.samples), 2)
            self.assertEqual(reader.coalesced, 999)
            self.assertEqual(reader.drops, {1: 100, 2: 0})
            self.assertEqual(reader.take()['event'], 'drops')
            self.assertEqual(reader.take()['event'], 'stopped')
            self.assertEqual(reader.take()['event'], 'graph')
            self.assertEqual(reader.take()['sequence'], 1000)
            self.assertEqual(reader.take()['subscription'], 2)
            self.assertIsNone(reader.take())
            self.assertLessEqual(reader.capture_bytes, pressure.RAW_CAPTURE_BYTES)
            self.assertLessEqual(reader.control_bytes, pressure.CONTROL_CAPTURE_BYTES)

    def test_full_control_fifo_is_bounded_and_pause_interrupts_retry(self):
        events = [{'event': 'started', 'subscription': 1, 'request': request} for request in range(1, 21)]
        with tempfile.TemporaryDirectory() as temporary:
            reader = pressure.Reader(stream(events), Path(temporary))
            deadline = time.monotonic() + 1
            while reader.control.qsize() < pressure.CONTROL_ITEMS and time.monotonic() < deadline:
                time.sleep(.005)
            self.assertEqual(reader.control.qsize(), pressure.CONTROL_ITEMS)
            reader.pause()
            frames_at_pause = reader.frames
            self.assertLessEqual(reader.control.qsize(), pressure.CONTROL_ITEMS)
            self.assertLessEqual(frames_at_pause, pressure.CONTROL_ITEMS + 1)
            reader.resume()
            reader.join_after_exit()
            self.assertEqual(reader.frames, 20)
            self.assertIsNone(reader.error)

    def test_identity_sequence_drop_regression_and_wire_limits_are_visible(self):
        bad_cases = [[sample(5, 1)], [sample(1, 1), sample(1, 1)],
                     [sample(1, 1, 9), sample(1, 2, 8)],
                     [{**sample(1, 1), 'generation': 80}]]
        for events in bad_cases:
            with self.subTest(events=events), tempfile.TemporaryDirectory() as temporary:
                reader = pressure.Reader(stream(events), Path(temporary))
                reader.thread.join(timeout=2)
                self.assertTrue(reader.finished.is_set())
                self.assertIsNotNone(reader.error)
        for raw in [b'x' * (pressure.MAX_LINE + 1), b'{}',
                    json.dumps({**sample(1, 1), 'payload': 'x' * 524288}).encode() + b'\n',
                    json.dumps({'event': 'started', 'message': 'x' * pressure.CONTROL_BYTES}).encode() + b'\n']:
            with self.subTest(length=len(raw)), tempfile.TemporaryDirectory() as temporary:
                reader = pressure.Reader(io.BytesIO(raw), Path(temporary))
                reader.thread.join(timeout=2)
                self.assertTrue(reader.finished.is_set())
                self.assertIsNotNone(reader.error)

    def test_declared_pressure_flags_are_finite_and_bounded(self):
        parser = pressure.argument_parser()
        args = parser.parse_args(['--label', 'round4-offline'])
        self.assertEqual((args.image_hz, args.stall_seconds), (30, 1.2))
        for flag, value in [('--image-hz', '9'), ('--image-hz', '31'), ('--image-hz', 'nan'),
                            ('--stall-seconds', 'inf'), ('--stall-seconds', '0.4'), ('--stall-seconds', '2.1')]:
            with self.subTest(flag=flag, value=value), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    parser.parse_args(['--label', 'round4-offline', flag, value])


if __name__ == '__main__':
    unittest.main()
