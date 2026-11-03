"""Offline bounded-reader checks; creates no ROS context, peers or subprocesses."""
import importlib.util
import io
import json
from pathlib import Path
import queue
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('observe_flood', ROOT / 'scripts/test-observe-session-flood.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class FloodReaderTests(unittest.TestCase):
    def test_full_queue_pause_acknowledges_without_losing_or_growing_frames(self):
        frames = b''.join(json.dumps({'event': 'observation', 'subscription': 1,
                                      'sequence': n, 'backend_drops': 0}).encode() + b'\n' for n in range(1, 101))
        with tempfile.TemporaryDirectory(prefix='studio-flood-reader-') as folder:
            reader = fixture.BoundedReader(io.BytesIO(frames), Path(folder))
            reader.pause()
            paused_count = reader.frames
            time.sleep(.03)
            self.assertEqual(reader.frames, paused_count)
            self.assertLessEqual(reader.events.qsize(), fixture.READER_ITEMS)
            self.assertLessEqual(reader.frames, fixture.READER_ITEMS + 1)
            reader.resume()
            sequences = []
            deadline = time.monotonic() + 2
            while len(sequences) < 100 and time.monotonic() < deadline:
                try:
                    sequences.append(reader.events.get(timeout=.05)['sequence'])
                except queue.Empty:
                    pass
            reader.join_after_exit()
            self.assertEqual(sequences, list(range(1, 101)))
            self.assertIsNone(reader.error)
            self.assertEqual(reader.flood_captures, 8)
            self.assertEqual(reader.omitted_frames, 92)
            self.assertLessEqual(reader.capture_bytes, fixture.CAPTURE_BYTES)

    def test_oversize_and_incomplete_frames_fail_before_queue_admission(self):
        for content in [b'x' * (fixture.MAX_LINE + 1), b'{"event":"error"}']:
            with self.subTest(bytes=len(content)), tempfile.TemporaryDirectory(prefix='studio-flood-reader-') as folder:
                reader = fixture.BoundedReader(io.BytesIO(content), Path(folder))
                self.assertTrue(reader.finished.wait(timeout=2))
                reader.thread.join(timeout=1)
                self.assertIn('Invalid bounded backend frame', reader.error)
                self.assertTrue(reader.events.empty())


if __name__ == '__main__':
    unittest.main()
