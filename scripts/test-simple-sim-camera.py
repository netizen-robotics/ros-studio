"""Offline camera endpoint/version migration tests; no ROS or network imports."""
import base64
import json
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[1]
contract = runpy.run_path(str(ROOT / 'fixtures/simple_sim/transport.py'))
relay = runpy.run_path(str(ROOT / 'scripts/simple-sim-relay.py'))
verify = runpy.run_path(str(ROOT / 'scripts/verify-simple-sim.py'))


def sample(typ, sequence=2, data=b'cdr'):
    return {'type': typ, 'sequence': sequence, 'cdr': base64.b64encode(data).decode('ascii')}


class CameraTransport(unittest.TestCase):
    def setUp(self):
        self.types = {topic: (typ,) for topic, typ in contract['TOPICS'].items()}
        self.seen = {topic: 0 for topic in self.types}

    def payload(self, version=2, samples=None):
        return json.dumps({'fixture_version': version, 'samples': samples or {}}).encode()

    def test_version1_seven_stream_compatibility_and_version2_eighth_stream(self):
        legacy = {topic: sample(typ) for topic, typ in contract['LEGACY_TOPICS'].items()}
        self.assertEqual(len(relay['decode_snapshot'](self.payload(1, legacy), self.seen, self.types)), 7)
        current = {**legacy, '/studio_sim/camera': sample('sensor_msgs/msg/Image')}
        decoded = relay['decode_snapshot'](contract['encode_snapshot'](current), self.seen, self.types)
        self.assertEqual({row[0] for row in decoded}, set(contract['TOPICS']))
        self.assertEqual(len(decoded), 8)
        with self.assertRaises(ValueError):
            relay['decode_snapshot'](self.payload(1, current), self.seen, self.types)
        # Startup subsets do not activate missing topics or create subscriptions.
        self.assertEqual(len(relay['decode_snapshot'](self.payload(2, legacy), self.seen, self.types)), 7)

    def test_camera_wrong_types_unknown_endpoints_versions_and_stale_sequences_rejected(self):
        cases = [self.payload(version=version) for version in [True, 0, 3, '2']]
        cases += [self.payload(samples={'/studio_sim/camera': sample('std_msgs/msg/String')}),
                  self.payload(samples={'/cmd_vel': sample('geometry_msgs/msg/Twist')}),
                  self.payload(samples={'/studio_sim/camera': sample('sensor_msgs/msg/Image', sequence=True)})]
        for payload in cases:
            with self.subTest(payload=payload), self.assertRaises(ValueError):
                relay['decode_snapshot'](payload, self.seen, self.types)
        self.seen['/studio_sim/camera'] = 10
        with self.assertRaises(ValueError):
            relay['decode_snapshot'](self.payload(samples={'/studio_sim/camera': sample('sensor_msgs/msg/Image', sequence=9)}), self.seen, self.types)
        self.assertEqual(self.seen['/studio_sim/camera'], 10)

    def test_sample_aggregate_canonical_base64_and_duplicate_json_bounds(self):
        topic, typ = '/studio_sim/camera', 'sensor_msgs/msg/Image'
        for encoded in ['AB==', 'AA==\n', '_w==', None, 'A' * 21849]:
            item = sample(typ)
            item['cdr'] = encoded
            with self.subTest(encoded=encoded), self.assertRaises(ValueError):
                relay['decode_snapshot'](self.payload(samples={topic: item}), self.seen, self.types)
        with self.assertRaises(ValueError):
            relay['decode_snapshot'](self.payload(samples={topic: sample(typ, data=b'x' * 16385)}), self.seen, self.types)
        with self.assertRaises(ValueError):
            relay['decode_snapshot'](b'x' * 131073, self.seen, self.types)
        with self.assertRaises(ValueError):
            relay['decode_snapshot'](b'{"fixture_version":1,"fixture_version":2,"samples":{}}', self.seen, self.types)
        with self.assertRaises(ValueError):
            contract['encode_snapshot']({topic: sample(typ, data=b'x' * 16385)})
        with self.assertRaises(ValueError):
            contract['encode_snapshot']({topic: sample(typ, data=b'x' * 16384) for topic, typ in contract['TOPICS'].items()})

    def test_verifier_accepts_current_typed_events_and_labels_camera_metadata_only(self):
        event = {'event': 'typed_sample', 'sample': {'contract_version': 2, 'topic': '/studio_sim/camera',
                 'message_type': 'sensor_msgs/msg/Image', 'value': {'kind': 'object', 'fields': [
                     {'name': name, 'value': {'kind': 'unsigned', 'value': str(value)}}
                     for name, value in [('width', 64), ('height', 48), ('step', 192)]
                 ] + [{'name': 'encoding', 'value': {'kind': 'text', 'value': 'rgb8', 'original_bytes': 4}}]}}}
        self.assertTrue(verify['received_typed_sample'](event, '/studio_sim/camera'))
        self.assertTrue(verify['camera_metadata_received']([event]))
        self.assertFalse(verify['received_typed_sample']({'event': 'sample', 'fields': [['data', 'preview']]}, '/studio_sim/camera'))
        event['sample']['value']['fields'][0]['value']['value'] = '640'
        self.assertFalse(verify['camera_metadata_received']([event]))


if __name__ == '__main__':
    unittest.main()
