import math
import unittest
import xml.etree.ElementTree as ET

import model


class Geometry(unittest.TestCase):
    def test_origin_cardinal_and_diagonal_wall_distances(self):
        ranges = model.scan(0, 0, 0, count=8)
        for i, distance in enumerate(ranges):
            self.assertAlmostEqual(distance, 3 if i % 2 == 0 else math.sqrt(18))

    def test_offset_ray_hits_wall(self):
        self.assertAlmostEqual(model.scan(1, 0, 0, count=4)[2], 2)
        self.assertAlmostEqual(model.scan(1, 0, math.pi / 2, count=4)[2], 3)

    def test_differential_wheels_agree_with_pose_derivative(self):
        for seconds in [0, 5, 30, 100]:
            x, y, yaw = model.pose(seconds)
            nx, ny, _ = model.pose(seconds + 1e-5)
            positions, velocities = model.wheels(seconds)
            speed = (velocities[0] + velocities[2]) * model.WHEEL_RADIUS / 2
            turn = (velocities[2] - velocities[0]) * model.WHEEL_RADIUS / model.EFFECTIVE_TRACK
            self.assertAlmostEqual((nx - x) / 1e-5, speed * math.cos(yaw), places=5)
            self.assertAlmostEqual((ny - y) / 1e-5, speed * math.sin(yaw), places=5)
            self.assertAlmostEqual(turn, model.TURN_RATE)
            self.assertEqual(positions, [v * seconds for v in velocities])
            self.assertTrue(all(0.05 < r < 10 for r in model.scan(x, y, yaw)))

    def test_description_matches_joint_names_and_unique_links(self):
        root = ET.fromstring(model.description())
        links = [link.attrib['name'] for link in root.findall('link')]
        self.assertEqual(len(links), len(set(links)))
        wheels = {joint.attrib['name'] for joint in root.findall('joint')
                  if joint.attrib['type'] == 'continuous'}
        self.assertEqual(wheels, set(model.JOINTS))
        for joint in root.findall('joint'):
            self.assertIn(joint.find('parent').attrib['link'], links)
            self.assertIn(joint.find('child').attrib['link'], links)


if __name__ == '__main__':
    unittest.main()
