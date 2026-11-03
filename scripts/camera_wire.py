"""Development-only rectified Camera snapshots; never ROS or TF evidence."""
from session_wire import camera_waiting

INFO_TOPIC = '/demo/calibrated/camera_info'
IMAGE_TOPIC = '/demo/calibrated/image_rect'


def snapshot(command, revision, image):
    packet = camera_waiting(command, revision)
    value = packet['sample']
    if value['owner']['info']['topic'] != INFO_TOPIC:
        return packet
    metadata = image['metadata']
    value['counters'].update(images=revision, calibrations=revision)
    if metadata['encoding'] not in ('mono8', 'rgb8', 'bgr8', 'rgba8', 'bgra8'):
        value.update(status='unsupported', diagnostic='Synthetic unsupported image encoding')
        return packet
    if not image['complete'] or image['original_bytes'] != metadata['step'] * metadata['height']:
        value.update(status='invalid_image', diagnostic='Synthetic invalid image content')
        return packet
    if value['owner']['topic'] != IMAGE_TOPIC:
        value.update(status='incompatible', diagnostic='Synthetic calibration is paired with /demo/calibrated/image_rect')
        return packet
    width, height = metadata['width'], metadata['height']
    fx, fy, cx, cy = 60., 60., width * .47, height * .48
    stamp = metadata['header_stamp']['seconds'] * 1000000000 + metadata['header_stamp']['nanoseconds']
    calibration = dict(stamp_ns=stamp, frame_id=metadata['frame_id'], width=width, height=height,
        distortion_model='plumb_bob', d=[], k=[fx, 0., cx, 0., fy, cy, 0., 0., 1.],
        r=[1., 0., 0., 0., 1., 0., 0., 0., 1.], p=[fx, 0., cx, 0., 0., fy, cy, 0., 0., 0., 1., 0.],
        binning_x=0, binning_y=0, roi=dict(x_offset=0, y_offset=0, width=0, height=0, do_rectify=False))
    value.update(status='receiving', frame=dict(image_sequence=revision, calibration_revision=1,
        image=image, calibration=calibration,
        transform=dict(status='ready', resolved_stamp_ns=stamp, components=[0., 0., 5., 1., 0., 0., 0.])))
    return packet
