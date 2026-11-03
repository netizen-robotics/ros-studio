"""Offline deterministic camera and native CDR tests; no ROS context/network."""
import base64
import json
import threading
import unittest

from builtin_interfaces.msg import Time
from rclpy.serialization import deserialize_message, serialize_message
from sensor_msgs.msg import Image
from std_msgs.msg import String

import model
from publisher import camera_message
from snapshot import Snapshot
from transport import encode_snapshot, MAX_CDR_BYTES, MAX_SNAPSHOT_BYTES


class CameraFixture(unittest.TestCase):
    def test_quadrants_counter_orientation_channel_order_alpha_and_padding(self):
        number = 0x0807060504030201
        for encoding, channels in model.CAMERA_ENCODINGS.items():
            with self.subTest(encoding=encoding):
                frame = model.camera(number, width=16, height=4, encoding=encoding, padding=3)
                self.assertEqual(len(frame.data), frame.step * frame.height)
                self.assertEqual(frame.step, 16 * channels + 3)
                for row in range(4):
                    self.assertEqual(frame.data[(row + 1) * frame.step - 3:(row + 1) * frame.step], b'\xa5' * 3)
                def pixel(x, y):
                    offset = y * frame.step + x * channels
                    return tuple(frame.data[offset:offset + channels])
                for index in range(8):
                    expected = (index + 1,) * min(channels, 3)
                    self.assertEqual(pixel(index, 0), (*expected, 255) if channels == 4 else expected)
                for x, y, rgba in [(8, 0, (255, 0, 255, 255)), (0, 3, (0, 255, 255, 255)),
                                   (1, 1, (255, 0, 0, 255)), (15, 1, (0, 255, 0, 192)),
                                   (1, 3, (0, 0, 255, 128)), (15, 3, (255, 255, 255, 64))]:
                    r, g, b, a = rgba
                    expected = {'mono8': ((77 * r + 150 * g + 29 * b + 128) // 256,),
                                'rgb8': (r, g, b), 'bgr8': (b, g, r),
                                'rgba8': rgba, 'bgra8': (b, g, r, a)}[encoding]
                    self.assertEqual(pixel(x, y), expected)
                self.assertEqual(frame.data, model.camera(number, 16, 4, encoding, 3, True).data)

    def test_default_frame_cdr_roundtrip_and_snapshot_fit_existing_bounds(self):
        frame = model.camera(42)
        message = camera_message(Time(sec=123, nanosec=456), frame)
        cdr = serialize_message(message)
        decoded = deserialize_message(cdr, Image)
        self.assertEqual(bytes(decoded.data), frame.data)
        self.assertEqual(decoded.header.frame_id, 'studio_sim/camera_optical')
        self.assertEqual((decoded.header.stamp.sec, decoded.header.stamp.nanosec), (123, 456))
        self.assertEqual((decoded.width, decoded.height, decoded.step, decoded.encoding), (64, 48, 192, 'rgb8'))
        self.assertEqual(len(frame.data), 9216)
        self.assertLessEqual(len(cdr), MAX_CDR_BYTES)
        payload = encode_snapshot({'/studio_sim/camera': {
            'type': 'sensor_msgs/msg/Image', 'sequence': 42, 'cdr': base64.b64encode(cdr).decode('ascii')}})
        self.assertLessEqual(len(payload), MAX_SNAPSHOT_BYTES)
        print(json.dumps({'default_camera_raw_bytes': len(frame.data), 'default_camera_cdr_bytes': len(cdr),
                          'camera_only_snapshot_bytes': len(payload), 'transport_sample_bound': MAX_CDR_BYTES,
                          'transport_snapshot_bound': MAX_SNAPSHOT_BYTES}))

    def test_frame_changes_are_bounded_and_unsupported_conversion_is_explicit(self):
        first, second = model.camera(255), model.camera(256)
        self.assertNotEqual(first.data[:24], second.data[:24])
        self.assertEqual(first.data[24:], second.data[24:])
        self.assertEqual(int.from_bytes(bytes(second.data[index * 3] for index in range(8)), 'little'), 256)
        for kwargs in [dict(frame_number=-1), dict(frame_number=True), dict(frame_number=2**64),
                       dict(frame_number=0, encoding='16UC1'), dict(frame_number=0, padding=-1),
                       dict(frame_number=0, width=2049), dict(frame_number=0, width=256, height=257),
                       dict(frame_number=0, padding=256 * 1024), dict(frame_number=0, big_endian=1)]:
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                model.camera(**kwargs)

    def test_snapshot_publish_validates_before_publishing_and_keeps_latest_only(self):
        class Publisher:
            def __init__(self):
                self.count = 0
            def publish(self, _message):
                self.count += 1
        # Avoid constructing Snapshot's HTTP server; exercise the real publish
        # method with only its bounded in-memory state and a local fake sink.
        snapshot = Snapshot.__new__(Snapshot)
        snapshot.lock, snapshot.latest, snapshot.sequence = threading.Lock(), {}, 0
        snapshot.payload = encode_snapshot({})
        publisher = Publisher()
        for number in range(20):
            snapshot.publish(publisher, '/studio_sim/camera', 'sensor_msgs/msg/Image',
                             camera_message(Time(sec=1), model.camera(number)))
        self.assertEqual(len(snapshot.latest), 1)
        self.assertEqual(snapshot.sequence, 20)
        self.assertEqual(publisher.count, 20)
        self.assertEqual(json.loads(snapshot.payload)['fixture_version'], 2)
        before = snapshot.payload
        with self.assertRaises(ValueError):
            snapshot.publish(publisher, '/cmd_vel', 'std_msgs/msg/String', String(data='command'))
        with self.assertRaises(ValueError):
            snapshot.publish(publisher, '/studio_sim/status', 'std_msgs/msg/String', String(data='x' * MAX_CDR_BYTES))
        self.assertEqual(publisher.count, 20)
        self.assertEqual(snapshot.payload, before)


if __name__ == '__main__':
    unittest.main()
