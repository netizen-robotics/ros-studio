"""Deterministic kinematic data fixture, inspired by Biggs chassis dimensions."""
import math
from dataclasses import dataclass

WHEEL_RADIUS = 0.0363
EFFECTIVE_TRACK = 0.365
WHEEL_X = 0.095
WHEEL_Y = 0.12605
TURN_RATE = 0.18
PATH_RADIUS = 0.8
ROOM_HALF_WIDTH = 3.0
JOINTS = ['front_left_wheel_joint', 'rear_left_wheel_joint',
          'front_right_wheel_joint', 'rear_right_wheel_joint']
CAMERA_FRAME = 'studio_sim/camera_optical'
CAMERA_WIDTH = 64
CAMERA_HEIGHT = 48
CAMERA_ENCODING = 'rgb8'
CAMERA_ENCODINGS = {'mono8': 1, 'rgb8': 3, 'bgr8': 3, 'rgba8': 4, 'bgra8': 4}


@dataclass(frozen=True)
class CameraFrame:
    width: int
    height: int
    encoding: str
    step: int
    is_bigendian: int
    frame_id: str
    frame_number: int
    data: bytes


def camera(frame_number, width=CAMERA_WIDTH, height=CAMERA_HEIGHT,
           encoding=CAMERA_ENCODING, padding=0, big_endian=False):
    """Known top-down quadrants, orientation markers and a uint64 counter strip.

    First eight pixels of the top row contain counter bytes, little endian, as
    gray values (alpha255); pixel8 is magenta. Bottom-left is cyan. Other pixels
    are red/green/blue/white quadrants with alpha255/192/128/64. Mono uses fixed
    integer luma. No unsupported-encoding conversion, resizing or calibration.
    """
    if type(frame_number) is not int or not 0 <= frame_number < 2**64:
        raise ValueError('Camera frame number must be a uint64')
    if encoding not in CAMERA_ENCODINGS:
        raise ValueError('Unsupported camera fixture encoding')
    if any(type(value) is not int for value in (width, height, padding)) or not 9 <= width <= 2048 or not 2 <= height <= 2048 or padding < 0:
        raise ValueError('Camera fixture requires width 9..2048, height 2..2048 and nonnegative padding')
    if type(big_endian) is not bool:
        raise ValueError('Camera byte order must be a bool')
    channels = CAMERA_ENCODINGS[encoding]
    step = width * channels + padding
    if width * height * 4 > 256 * 1024 or step * height > 256 * 1024:
        raise ValueError('Camera fixture exceeds its 256 KiB raw/RGBA budget')
    counter = frame_number.to_bytes(8, 'little')
    data = bytearray()
    colors = [(255, 0, 0, 255), (0, 255, 0, 192), (0, 0, 255, 128), (255, 255, 255, 64)]
    for y in range(height):
        for x in range(width):
            rgba = colors[(2 if y >= height // 2 else 0) + (1 if x >= width // 2 else 0)]
            if y == 0 and x < 8:
                rgba = (counter[x], counter[x], counter[x], 255)
            elif y == 0 and x == 8:
                rgba = (255, 0, 255, 255)
            elif y == height - 1 and x == 0:
                rgba = (0, 255, 255, 255)
            r, g, b, a = rgba
            if encoding == 'mono8':
                data.append((77 * r + 150 * g + 29 * b + 128) // 256)
            elif encoding == 'rgb8':
                data.extend((r, g, b))
            elif encoding == 'bgr8':
                data.extend((b, g, r))
            elif encoding == 'rgba8':
                data.extend((r, g, b, a))
            else:
                data.extend((b, g, r, a))
        data.extend(b'\xa5' * padding)
    return CameraFrame(width, height, encoding, step, int(big_endian), CAMERA_FRAME,
                       frame_number, bytes(data))


def pose(seconds):
    yaw = TURN_RATE * seconds
    return PATH_RADIUS * math.sin(yaw), PATH_RADIUS * (1 - math.cos(yaw)), yaw


def wheels(seconds):
    speed = PATH_RADIUS * TURN_RATE
    left = (speed - TURN_RATE * EFFECTIVE_TRACK / 2) / WHEEL_RADIUS
    right = (speed + TURN_RATE * EFFECTIVE_TRACK / 2) / WHEEL_RADIUS
    velocities = [left, left, right, right]
    return [v * seconds for v in velocities], velocities


def scan(x, y, yaw, count=180):
    """Ray intersection with a square empty room, from the laser origin."""
    ranges = []
    for i in range(count):
        angle = yaw - math.pi + i * 2 * math.pi / count
        dx, dy = math.cos(angle), math.sin(angle)
        hits = []
        for origin, direction in [(x, dx), (y, dy)]:
            if abs(direction) > 1e-12:
                wall = ROOM_HALF_WIDTH if direction > 0 else -ROOM_HALF_WIDTH
                hits.append((wall - origin) / direction)
        ranges.append(min(t for t in hits if t > 0))
    return ranges


def description():
    links = ['<link name="studio_sim/base_footprint"/>',
             '<link name="studio_sim/base_link"><visual><geometry>'
             '<box size="0.350 0.270 0.086"/></geometry></visual></link>',
             '<joint name="base_height" type="fixed"><parent link="studio_sim/base_footprint"/>'
             '<child link="studio_sim/base_link"/><origin xyz="0 0 0.0363"/></joint>',
             '<link name="studio_sim/laser"/>',
             '<joint name="laser_mount" type="fixed"><parent link="studio_sim/base_link"/>'
             '<child link="studio_sim/laser"/><origin xyz="0 0 0.0529"/></joint>']
    for name, x, y in zip(JOINTS, [WHEEL_X, -WHEEL_X] * 2, [WHEEL_Y] * 2 + [-WHEEL_Y] * 2):
        link = 'studio_sim/' + name.removesuffix('_joint')
        links.extend([
            f'<link name="{link}"><visual><origin rpy="1.57079632679 0 0"/>'
            f'<geometry><cylinder radius="{WHEEL_RADIUS}" length="0.020"/></geometry></visual></link>',
            f'<joint name="{name}" type="continuous"><parent link="studio_sim/base_link"/>'
            f'<child link="{link}"/><origin xyz="{x} {y} 0"/><axis xyz="0 1 0"/></joint>',
        ])
    return '<robot name="studio_biggs_fixture">' + ''.join(links) + '</robot>'
