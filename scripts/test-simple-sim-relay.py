"""Offline fixture transport rejection checks; no ROS or network traffic."""
import base64
import json
from pathlib import Path
import runpy
import unittest

decode = runpy.run_path(str(Path(__file__).with_name('simple-sim-relay.py')))['decode_snapshot']


class RelayTransport(unittest.TestCase):
    def payload(self, topic='/studio_sim/status', typ='std_msgs/msg/String', sequence=2, data=b'cdr'):
        return json.dumps({'fixture_version': 1, 'samples': {
            topic: {'type': typ, 'sequence': sequence, 'cdr': base64.b64encode(data).decode()}}}).encode()

    def test_new_and_already_seen_samples(self):
        types = {'/studio_sim/status': ('std_msgs/msg/String',)}
        seen = {'/studio_sim/status': 1}
        self.assertEqual(decode(self.payload(), seen, types), [('/studio_sim/status', 2, b'cdr')])
        self.assertEqual(decode(self.payload(sequence=1), seen, types), [])
        self.assertEqual(seen['/studio_sim/status'], 1)

    def test_unexpected_types_commands_versions_restarts_and_sizes_rejected(self):
        types = {'/studio_sim/status': ('std_msgs/msg/String',)}
        seen = {'/studio_sim/status': 1}
        cases = [self.payload(topic='/cmd_vel'), self.payload(typ='geometry_msgs/msg/Twist'),
                 self.payload(sequence=0), self.payload(sequence=True), self.payload(sequence=-1),
                 self.payload(data=b'x' * 16385), b'x' * 131073,
                 b'{"fixture_version": 9, "samples": {}}', b'[]']
        for payload in cases:
            with self.subTest(size=len(payload)), self.assertRaises(ValueError):
                decode(payload, seen, types)


if __name__ == '__main__':
    unittest.main()
