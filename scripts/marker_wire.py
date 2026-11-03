"""Development-only synthetic Marker snapshot1 values; no ROS/native TF proof."""
import base64
import hashlib
import json
import time
from pathlib import Path

CAPABILITIES = dict(contract_version=1, max_document_bytes=16777216, max_chunk_bytes=196608,
                    max_chunks=86, max_sample_bytes=524288, max_owners=2, max_markers=256,
                    max_points_per_marker=4096, max_operations=256, max_batch_points=16384,
                    max_retained_points=65536, max_text_bytes_per_marker=4096,
                    max_resource_bytes_per_marker=4096, max_retained_text_bytes=65536,
                    max_retained_resource_bytes=65536, max_diagnostics=64,
                    supported_kinds=list(range(12)), max_hz=20)


def owner(command):
    config = command['configuration']
    return dict(generation=command['generation'], subscription=command['subscription'],
                topic=config['topic'], message_type=config['message_type'], fixed_frame=config['spatial']['fixed_frame'])


def rich_markers(config, stamp, profile):
    """All pinned kinds plus confined/unavailable assets; presentation fixture only."""
    points = [[-.6, -.35, 0.], [0., .4, .2], [.6, -.35, .1]]
    colors = [[.9, .3, .2, 1.], [.2, .8, .55, 1.], [.3, .5, .95, 1.]]
    asset = Path(__file__).resolve().parents[1] / 'fixtures/robot_visualizer/materials/panels.obj'
    geometries = [dict(kind='arrow', points=[]), dict(kind='cube'), dict(kind='sphere'),
                  dict(kind='cylinder'), dict(kind='line_strip', points=points, colors=colors),
                  dict(kind='line_list', points=points[:2], colors=colors[:2]),
                  dict(kind='cube_list', points=points, colors=colors),
                  dict(kind='sphere_list', points=points, colors=colors),
                  dict(kind='points', points=points, colors=colors),
                  dict(kind='text', text='Marker text · 机器人\nDepth + lifetime'),
                  dict(kind='mesh', resource=asset.as_uri(), use_embedded_materials=True),
                  dict(kind='triangles', points=points, colors=colors),
                  dict(kind='mesh', resource='package://marker_demo/materials/panels.obj', use_embedded_materials=True),
                  dict(kind='mesh', resource=Path('/private/tmp/unapproved-marker.obj').as_uri(), use_embedded_materials=True)]
    markers=[]
    for i, geometry in enumerate(geometries):
        scale=[.65, .65, .65]
        if geometry['kind'] in ('line_strip','line_list'): scale=[.1,.1,.1]
        if geometry['kind'] in ('cube_list','sphere_list','points'): scale=[.25,.25,.25]
        if geometry['kind']=='text': scale=[1.,1.,.35]
        namespace='native_demo/' + (geometry['kind'] if i!=8 else 'long_namespace_' + 'sensor_'*28)
        markers.append(dict(key=dict(namespace=namespace,id=i),frame_id=config['spatial']['fixed_frame'],
            stamp_ns=stamp,pose=[(i%4-1.5)*2.6,(i//4-1.5)*2.2,.5,0.,0.,0.,1.],
            scale=scale,color=[.85,.55,.2,1.],lifetime_ns=15000000000 if profile=='finite' and i==1 else 0,
            frame_locked=i==2,geometry=geometry))
    return sorted(markers, key=lambda m: (m['key']['namespace'], m['key']['id']))


def snapshots(command, revision, profile='cube'):
    config = command['configuration']
    stamp = time.time_ns()
    key = dict(namespace='synthetic', id=1)
    marker = dict(key=key, frame_id=config['spatial']['fixed_frame'], stamp_ns=stamp,
                  pose=[0., 0., 0., 0., 0., 0., 1.], scale=[1., 1., 1.], color=[0.9, 0.3, 0.2, 1.],
                  lifetime_ns=0, frame_locked=False, geometry=dict(kind='cube'))
    counters = dict.fromkeys(('batches', 'adds', 'deletes', 'expired', 'rejected', 'duplicate_keys', 'stale',
                             'source_gaps', 'source_faults', 'lookup_timeouts'), 0)
    counters.update(batches=1, adds=1)
    source_counters = dict.fromkeys(('scans_received', 'pending_replaced', 'lookup_timeouts', 'tf_dynamic_batches',
                                    'tf_static_batches', 'tf_accepted', 'tf_rejected', 'duplicate_statics', 'native_errors'), 0)
    markers = [marker] if profile == 'cube' else rich_markers(config, stamp, profile)
    if profile == 'maximum':
        # Same declared workload as marker_performance_tests.rs. These are
        # synthetic canonical states, never ROS or native TF evidence.
        geometries = [dict(kind='points', points=[[.1, .2, .3]] * 4096, colors=[]) for _ in range(16)]
        geometries += [dict(kind='text', text='A' * 4096) for _ in range(16)]
        if config['topic'] == '/demo/markers':
            geometries.append(dict(kind='mesh', resource='maximum.obj', use_embedded_materials=False))
        geometries += [dict(kind='arrow', points=[]) for _ in range(256-len(geometries))]
        markers = [dict(key=dict(namespace='maximum', id=i), frame_id=config['spatial']['fixed_frame'],
                        stamp_ns=0, pose=[0., 0., 0., 0., 0., 0., 1.], scale=[.1]*3, color=[1.]*4,
                        lifetime_ns=0, frame_locked=False, geometry=g) for i,g in enumerate(geometries)]
    counters.update(adds=len(markers), batches=revision)
    version = 1 if profile == 'maximum' else revision
    entries = [dict(marker=m, version=version, transform=(dict(kind='pending')
               if profile == 'missing' and m['key']['id'] == 2 else dict(kind='resolved', pose=dict(
               status='ready', resolved_stamp_ns=stamp, components=[0., 0., 0., 0., 0., 0., 1.])))) for m in markers]
    state = dict(entries=entries, diagnostics=[], counters=counters, source_counters=source_counters,
                 known_history_gap=False, loss_support=dict(kind='unknown'))
    raw = json.dumps(state, ensure_ascii=False, allow_nan=False, separators=(',', ':')).encode()
    assert len(raw) <= CAPABILITIES['max_document_bytes']
    count = (len(raw)+196607)//196608
    header = dict(contract_version=1, owner=owner(command), revision=revision, source_revision=revision,
                  captured_at_unix_ns=stamp, manifest=[dict(key=m['key'], version=version, lifetime=(dict(kind='expires', remaining_ns=m['lifetime_ns'], at_unix_ns=stamp+m['lifetime_ns']) if m['lifetime_ns'] else dict(kind='forever'))) for m in markers],
                  document_bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
    return [dict(kind='chunk', header=header, index=i, count=count,
                 data_base64=base64.b64encode(raw[i*196608:(i+1)*196608]).decode()) for i in range(count)]


def snapshot(command, revision, profile='cube'):
    """Retained single-chunk helper for the small offline fixtures."""
    chunks = snapshots(command, revision, profile)
    assert len(chunks) == 1
    return chunks[0]


def clear(command, revision):
    return dict(kind='clear', contract_version=1, owner=owner(command), revision=revision,
                reason='local_clear', known_history_gap=False)
