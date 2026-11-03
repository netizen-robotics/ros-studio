"""Offline Image fixture bounds, raw layout and scheduling checks; no ROS peers."""
import base64
import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('image_acceptance', ROOT / 'scripts/test-image-session.py')
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)
fixture = acceptance.WORKFLOW


class ImageFixtureTests(unittest.TestCase):
    def test_five_encodings_preserve_counter_padding_orientation_and_alpha(self):
        for encoding, channels in [('rgb8', 3), ('mono8', 1), ('bgr8', 3), ('rgba8', 4), ('bgra8', 4)]:
            with self.subTest(encoding=encoding):
                frame = fixture.camera(encoding, 0x0102030405060708)
                self.assertEqual((frame.width, frame.height), (64, 48))
                self.assertEqual(frame.step, 64 * channels + fixture.PADDING[encoding])
                self.assertEqual(len(frame.data), frame.step * 48)
                self.assertEqual(fixture.frame_number({'encoding': encoding}, frame.data), frame.frame_number)
                for row in range(48):
                    self.assertEqual(frame.data[row * frame.step + 64 * channels:(row + 1) * frame.step], b'\xa5' * fixture.PADDING[encoding])
                if channels == 4:
                    self.assertEqual(frame.data[frame.step + 40 * channels + 3], 192)
                    self.assertEqual(frame.data[30 * frame.step + 20 * channels + 3], 128)
                    self.assertEqual(frame.data[30 * frame.step + 40 * channels + 3], 64)
                # Known magenta top marker and cyan bottom marker independently
                # distinguish vertical orientation from a byte-perfect copy.
                if encoding == 'rgb8':
                    self.assertEqual(frame.data[24:27], bytes([255, 0, 255]))
                    self.assertEqual(frame.data[-frame.step:-frame.step + 3], bytes([0, 255, 255]))

    def test_maximum_unsupported_malformed_and_format_variants(self):
        maximum = fixture.camera('max_rgba', 9)
        self.assertEqual(len(maximum.data), 262144)
        self.assertEqual((maximum.width, maximum.height, maximum.step), (256, 256, 1024))
        unsupported = fixture.camera('unsupported', 10)
        self.assertEqual(unsupported.encoding, '16UC1')
        self.assertEqual(len(unsupported.data), unsupported.height * unsupported.step)
        self.assertEqual(fixture.frame_number({'encoding': '16UC1'}, unsupported.data), 10)
        malformed = fixture.camera('malformed', 1)
        self.assertEqual(len(malformed.data) + 1, malformed.height * malformed.step)
        variants = [fixture.camera('format_changes', i) for i in range(10)]
        self.assertEqual({frame.encoding for frame in variants}, set(fixture.ENCODINGS))
        self.assertEqual({(frame.width, frame.height) for frame in variants}, {(64, 48), (48, 32)})

    def test_scheduling_has_no_catchup_and_status_is_independent(self):
        schedule = fixture.Schedule(10, 10, 2)
        self.assertEqual(set(schedule.due(10)), {'images', 'maximum', 'status', 'stats'})
        self.assertEqual(schedule.due(10), [])
        self.assertEqual(schedule.due(10.11), ['images'])
        self.assertEqual(set(schedule.due(1000)), {'images', 'maximum', 'status', 'stats'})
        self.assertEqual(schedule.due(1000), [])
        self.assertEqual(len(schedule.next), 4)
        self.assertGreater(schedule.wait(1000), 0)
        self.assertLessEqual(schedule.wait(1000), .2)

    def test_invalid_flags_precede_guard_and_ros_import(self):
        parser = fixture.argument_parser()
        for flag, value in [('--hz', 'nan'), ('--hz', '0'), ('--hz', '30.1'), ('--max-hz', 'inf'), ('--seconds', '0'), ('--seconds', '3601')]:
            with self.subTest(flag=flag, value=value), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    parser.parse_args([flag, value])
        for seconds, hz, maximum in [(0, 10, 2), (1, float('nan'), 2), (1, 10, 31)]:
            with patch.dict(fixture.HELPERS, {'checked_environment': lambda: self.fail('unsafe input reached ROS guard')}):
                with self.assertRaises(ValueError):
                    fixture.publisher(seconds, hz, maximum)

    def test_raw_acceptance_rejects_changed_metadata_and_payload(self):
        frame = fixture.camera('rgba8', 12)
        metadata = {key: getattr(frame, key) for key in ('width', 'height', 'encoding', 'step', 'frame_id')}
        metadata.update(is_bigendian=bool(frame.is_bigendian), header_stamp={'seconds': 1, 'nanoseconds': 2})
        sample = {'contract_version': 1, 'message_type': 'sensor_msgs/msg/Image', 'topic': fixture.PREFIX + 'rgba8',
                  'complete': True, 'metadata': metadata, 'original_bytes': len(frame.data),
                  'data_base64': base64.b64encode(frame.data).decode()}
        self.assertEqual(acceptance.validate_compact({'sample': sample}, 'rgba8')[0], 12)
        metadata['width'] = 63
        with self.assertRaises(AssertionError):
            acceptance.validate_compact({'sample': sample}, 'rgba8')
        metadata['width'] = 64
        sample['data_base64'] = base64.b64encode(frame.data[:-1] + b'\x00').decode()
        with self.assertRaises(AssertionError):
            acceptance.validate_compact({'sample': sample}, 'rgba8')


if __name__ == '__main__':
    unittest.main()
