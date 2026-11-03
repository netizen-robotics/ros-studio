#!/bin/sh
""":"
exec "$(dirname "$0")/../.pixi/envs/default/bin/python" "$0" "$@"
":"""
"""Development-only session7 scan/TF fixture; no ROS or network traffic.

Launch as STUDIO_BACKEND=$PWD/scripts/rviz-scene-fixture.py, select /demo/scan,
map, /tf and /tf_static, verify system-clock intent, then Enable display.
STUDIO_RVIZ_SCENE_MODE: tilted (default), freeze, missing, recover,
occlusion-near, occlusion-far.
Tilted/freeze use one tilted plane plus the world grid/axes as a noncoplanar
scene; they do not claim a single LaserScan is nonplanar. Freeze repeats the
same geometry with fresh acquisition stamps. Recover clears geometry for
five seconds, restores it for five seconds, and repeats. Existing UI fixture
modes, command logging, exit-request and bounded stream controls are retained.

Occlusion modes require Reset camera: orbit 0.5 rad, elevation 0.8 rad,
distance 12 m, target [0,0,0]. One range-1 ray uses identity rotation and
translation to place it 0.5 m toward/away from that eye along the ray through
the blue Z-axis point [0,0,0.5]. Both project onto the same axis pixel, but
near cyan should cover blue and far cyan should be hidden by blue. Camera
movement invalidates that alignment. Acquisition stamps remain fresh; these
are synthetic spatial1 results and do not establish native tf2 behavior.
"""
import importlib.util
import math
import os
from pathlib import Path
import time


def configure_fixture():
    path = Path(__file__).with_name('ui-workspace-fixture.py')
    spec = importlib.util.spec_from_file_location('rviz_session_fixture', path)
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    mode = os.environ.get('STUDIO_RVIZ_SCENE_MODE', 'tilted')
    if mode not in {'tilted', 'freeze', 'missing', 'recover', 'occlusion-near', 'occlusion-far'}:
        raise SystemExit('Unknown STUDIO_RVIZ_SCENE_MODE')
    original = fixture.spatial_sample
    started = time.monotonic()

    def spatial_sample(stream, now, base_mode):
        unavailable = mode == 'missing' or (mode == 'recover' and int((now - started) / 5) % 2 == 0)
        sample = original(stream, now, 'spatial-missing' if unavailable else base_mode)
        if sample is None or sample['tf']['status'] != 'ready':
            return sample
        if mode in {'occlusion-near', 'occlusion-far'}:
            axis = [0.0, 0.0, 0.5]
            eye = [12 * math.cos(0.8) * math.cos(0.5),
                   12 * math.cos(0.8) * math.sin(0.5), 12 * math.sin(0.8)]
            toward_eye = [eye[i] - axis[i] for i in range(3)]
            length = math.sqrt(sum(value * value for value in toward_eye))
            offset = 0.5 if mode == 'occlusion-near' else -0.5
            point = [axis[i] + offset * toward_eye[i] / length for i in range(3)]
            # Scan ray is [1,0,0]; identity rotation plus this translation
            # yields exactly the chosen world point after projection.
            sample['tf']['components'] = [point[0] - 1.0, point[1], point[2], 0.0, 0.0, 0.0, 1.0]
            fields = sample['scan']['value']['fields']
            replacements = {'angle_min': fixture.number(0.0), 'angle_max': fixture.number(0.0),
                            'angle_increment': fixture.number(0.1),
                            'ranges': fixture.array([fixture.number(1.0)])}
            for field in fields:
                if field['name'] in replacements:
                    field['value'] = replacements[field['name']]
            return sample
        # Roll/pitch gives observable Z variation; all components are finite
        # and normalized. This is protocol geometry, not a native tf2 result.
        roll = 0.35 if mode == 'freeze' else 0.35 + 0.12 * math.sin(stream['acquisition'] / 10)
        pitch = 0.65
        sr, cr = math.sin(roll / 2), math.cos(roll / 2)
        sp, cp = math.sin(pitch / 2), math.cos(pitch / 2)
        sample['tf']['components'] = [0.0, 0.0, 1.5, sr * cp, cr * sp, -sr * sp, cr * cp]
        return sample

    fixture.spatial_sample = spatial_sample
    return fixture


if __name__ == '__main__':
    try:
        configure_fixture().main()
    except BrokenPipeError:
        pass  # Closing the desktop pipe cancels this synthetic session.
