"""Current pose_snapshot1 capability constants for development fixtures."""
CAPABILITIES = dict(contract_version=1, max_sample_bytes=524288,
                    max_control_bytes=4096, max_decoded_bytes=8192,
                    max_history=256, max_pending=16, max_owners=2,
                    max_owners_per_kind=1, max_hz=20,
                    retry_ns=50000000, timeout_ns=1000000000)

TYPES = {'pose_with_covariance': 'geometry_msgs/msg/PoseWithCovarianceStamped',
         'odometry': 'nav_msgs/msg/Odometry'}


def configuration(output):
    if not isinstance(output, dict):
        return None
    if set(output) != {'pose_snapshot'} or not isinstance(output['pose_snapshot'], dict):
        raise ValueError('Invalid pose output')
    value = output['pose_snapshot']
    if set(value) != {'kind', 'history'} or value['kind'] not in TYPES:
        raise ValueError('Invalid pose kind/history grammar')
    history = value['history']
    if value['kind'] == 'pose_with_covariance':
        if history is not None:
            raise ValueError('Covariance pose has no history')
    elif (not isinstance(history, dict)
          or set(history) != {'keep', 'position_tolerance_um', 'angle_tolerance_urad'}
          or any(type(v) is not int for v in history.values())
          or not 1 <= history['keep'] <= 256
          or not 0 <= history['position_tolerance_um'] <= 1000000000
          or not 0 <= history['angle_tolerance_urad'] <= 3141592):
        raise ValueError('Invalid Odometry history')
    return value


def owner(command):
    c = command['configuration']
    p = configuration(c['output'])
    return dict(generation=command['generation'], subscription=command['subscription'],
                kind=p['kind'], topic=c['topic'], fixed_frame=c['spatial']['fixed_frame'])


def clear(command, stream):
    stream['pose_epoch'] = stream.get('pose_epoch', 1) + 1
    stream['pose_revision'] = stream.get('pose_revision', 0) + 1
    stream['pose_trail'] = []
    stream['pose_reset'] = 'local_clear'
    return dict(contract_version=1, owner=owner(command), revision=stream['pose_revision'],
                continuity_epoch=stream['pose_epoch'], reason='local_clear')


def snapshot(command, stream):
    import math
    import time
    c = command['configuration']
    p = configuration(c['output'])
    n = stream.get('pose_receipt', 0) + 1
    stream['pose_receipt'] = n
    stream['pose_revision'] = stream.get('pose_revision', 0) + 1
    t = n * .025
    components = [2 * math.cos(t), 2 * math.sin(t), .2 * math.sin(t / 3),
                  0., 0., math.sin(t / 2), math.cos(t / 2)]
    covariance = [0.] * 36
    for i, value in enumerate([.09, .04, .01, .01, .04, .09]):
        covariance[i * 7] = value
    covariance[1] = covariance[6] = .03
    stamp = time.time_ns()
    entry = dict(sequence=n, acquisition_stamp_ns=stamp, components=components,
                 covariance=covariance, transform=dict(status='ready', resolved_stamp_ns=stamp,
                                                      components=[0., 0., 0., 0., 0., 0., 1.]))
    trail = stream.setdefault('pose_trail', [])
    h = p['history']
    history = None if h is None else dict(keep=h['keep'], position_tolerance_m=h['position_tolerance_um'] / 1e6,
                                        angle_tolerance_rad=h['angle_tolerance_urad'] / 1e6)
    if h is not None:
        old = trail[-1] if trail else None
        angle = 0. if old is None else 2 * math.acos(min(1., abs(sum(a * b for a, b in zip(old['components'][3:], components[3:])))))
        if old is None or math.dist(old['components'][:3], components[:3]) >= history['position_tolerance_m'] or angle >= history['angle_tolerance_rad']:
            trail.append(entry)
        else:
            stream['pose_filtered'] = stream.get('pose_filtered', 0) + 1
        stream['pose_evicted'] = stream.get('pose_evicted', 0) + max(0, len(trail) - h['keep'])
        del trail[:-h['keep']]
    value = dict(contract_version=1, owner=owner(command), revision=stream['pose_revision'],
                 continuity_epoch=stream.get('pose_epoch', 1),
                 source=dict(frame_id=c['spatial']['fixed_frame'], child_frame_id='base_link' if h is not None else None),
                 history=history, latest=entry, trail=list(trail), last_reset=stream.get('pose_reset'),
                 known_history_gap=False, counters=dict(received=n, duplicates=0, filtered=stream.get('pose_filtered', 0), evicted=stream.get('pose_evicted', 0),
                                                       pending_dropped=0, lookup_timeouts=0, known_source_gaps=0,
                                                       resets=stream.get('pose_epoch', 1) - 1))
    return dict(kind='snapshot', sample=value)
