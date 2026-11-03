"""Development fixtures for the bounded cloud2 little-endian XYZ contract."""
import base64
import math
import struct


def encode_points(points):
    if len(points) > 8192:
        raise ValueError('Cloud point limit exceeded')
    raw = bytearray()
    for point in points:
        if len(point) != 3 or any(not math.isfinite(n) or abs(n) > 1e6 for n in point):
            raise ValueError('Invalid cloud XYZ')
        raw.extend(struct.pack('<fff', *point))
    return dict(count=len(points), data_base64=base64.b64encode(raw).decode('ascii'))


def decode_points(wire):
    if set(wire) != {'count', 'data_base64'} or type(wire['count']) is not int or not 0 <= wire['count'] <= 8192:
        raise ValueError('Invalid cloud point envelope')
    encoded = wire['data_base64']
    expected = wire['count'] * 12
    if not isinstance(encoded, str) or len(encoded) != 4 * ((expected + 2) // 3):
        raise ValueError('Invalid packed XYZ length')
    raw = base64.b64decode(encoded, validate=True)
    if len(raw) != expected or base64.b64encode(raw).decode('ascii') != encoded:
        raise ValueError('Invalid packed XYZ encoding')
    points = [list(p) for p in struct.iter_unpack('<fff', raw)]
    if any(not math.isfinite(n) or abs(n) > 1e6 for p in points for n in p):
        raise ValueError('Invalid cloud XYZ')
    return points
