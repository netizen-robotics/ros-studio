"""Tools protocol/native ROS regression runner. Synthetic localhost peers only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import subprocess
import sys
import threading
import time
from session_wire import VERSION

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--label', required=True)
    parser.add_argument('--stage', default='topics', choices=['topics', 'services', 'actions', 'parameters', 'custom', 'nav2', 'all'])
    parser.add_argument('--custom-bundle', type=Path)
    parser.add_argument('--custom-install', type=Path)
    args = parser.parse_args()
    if args.stage=='custom' and not (args.custom_bundle and args.custom_install):
        raise SystemExit('Custom acceptance requires a bundle and generated peer install')
    if Path(sys.prefix).resolve() != (ROOT / '.pixi/envs/default').resolve():
        raise SystemExit('Use scripts/dev project Python')
    if not args.label.replace('-', '').isalnum():
        raise SystemExit('Use an unused alphanumeric/hyphen evidence label')
    evidence = ROOT / 'artifacts/feasibility/evidence' / args.label
    evidence.mkdir(parents=True, exist_ok=False)
    report = dict(os=platform.platform(), cpu=platform.machine(), distro='jazzy',
                  versions={'rust': '1.92.0', 'rclrs': '0.8.0'}, command=sys.argv,
                  executable_sha256=hashlib.sha256((ROOT / 'target/debug/studio-backend').read_bytes()).hexdigest(),
                  cases=[], evidence=str(evidence.relative_to(ROOT)), outcome='failed')
    report['versions']['lockfiles'] = {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in ['Cargo.lock', 'pixi.lock']}
    report['versions']['native_packages'] = [{key: data[key] for key in ['name', 'version', 'build']} for path in sorted((ROOT / '.pixi/envs/default/conda-meta').glob('*.json')) if (data := json.loads(path.read_text()))['name'].startswith(('ros-jazzy-rcl', 'ros-jazzy-rmw', 'ros-jazzy-rosidl', 'fast', 'cyclonedds'))]
    os.environ['ROS_LOG_DIR'] = str(evidence / 'ros-logs')
    rmw = os.environ['RMW_IMPLEMENTATION']
    report['middleware_pair'] = [rmw, rmw]
    os.environ.update(ROS_DOMAIN_ID='187', ROS_LOCALHOST_ONLY='1', ROS_AUTOMATIC_DISCOVERY_RANGE='LOCALHOST')
    for key in ['ROS_STATIC_PEERS', 'ROS_DISCOVERY_SERVER', 'ROS_SUPER_CLIENT', 'CYCLONEDDS_URI',
                'FASTDDS_DEFAULT_PROFILES_FILE', 'FASTRTPS_DEFAULT_PROFILES_FILE', 'FASTDDS_ENVIRONMENT_FILE']:
        os.environ.pop(key, None)
    process = None
    log = None
    node = None
    fixture_executor = None
    action_server = None
    custom_handles = []
    try:
        import rclpy
        from std_msgs.msg import String
        rclpy.init()
        node = rclpy.create_node('studio_tools_fixture', namespace='/studio_fixture')
        if args.stage in ['parameters', 'all']:
            import importlib.util
            spec = importlib.util.spec_from_file_location('tools_peers', ROOT / 'scripts/tools-peers.py')
            parameter_peers = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(parameter_peers)
            parameter_peers.create_parameters(node)
        if args.stage in ['services', 'all']:
            from example_interfaces.srv import AddTwoInts
            def addition(request, result):
                if request.a == -99:
                    time.sleep(8)
                result.sum = request.a + request.b
                return result
            server = node.create_service(AddTwoInts, '/studio_fixture/tools_add', addition)
        if args.custom_bundle:
            import importlib.util
            sys.path.insert(0, str(args.custom_install / 'lib/python3.12/site-packages'))
            spec = importlib.util.spec_from_file_location('tools_custom_peers', ROOT / 'scripts/tools-peers.py')
            custom_peers = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(custom_peers)
            custom_received, custom_handles = custom_peers.create_custom(node)
        if args.stage in ['actions', 'all', 'nav2'] or args.custom_bundle:
            import importlib.util
            spec = importlib.util.spec_from_file_location('tools_peers', ROOT / 'scripts/tools-peers.py')
            peers = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(peers)
            action_server = peers.create_nav2_action(node) if args.stage == 'nav2' else peers.create_action(node)
            from rclpy.executors import MultiThreadedExecutor
            fixture_executor = MultiThreadedExecutor(num_threads=3)
            fixture_executor.add_node(node)
            fixture_thread = threading.Thread(target=fixture_executor.spin, daemon=True)
            fixture_thread.start()
        received = []
        sub = node.create_subscription(String, '/studio_fixture/tools_text', lambda msg: received.append(msg.data), 10)
        log = (evidence / 'backend-stderr.log').open('w')
        backend_env = os.environ.copy()
        backend_env['AMENT_PREFIX_PATH'] = str(ROOT / '.pixi/envs/default')
        backend_env['DYLD_LIBRARY_PATH'] = str(ROOT / '.pixi/envs/default/lib')
        report['backend_environment'] = {key: backend_env[key] for key in ['AMENT_PREFIX_PATH', 'DYLD_LIBRARY_PATH', 'ROS_DOMAIN_ID', 'ROS_LOCALHOST_ONLY', 'ROS_AUTOMATIC_DISCOVERY_RANGE', 'RMW_IMPLEMENTATION']}
        process = subprocess.Popen([str(ROOT / 'target/debug/studio-backend'), '--observe-session'], cwd=ROOT,
                                   env=backend_env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
        events = queue.Queue()
        transcript = []
        def read_protocol():
            for line in process.stdout:
                transcript.append(line)
                try:
                    events.put(json.loads(line))
                except ValueError:
                    events.put({'event': 'invalid', 'line': line})
        report['backend_pid'] = process.pid
        reader = threading.Thread(target=read_protocol, daemon=True)
        reader.start()
        backlog = []
        def wait(predicate, seconds=10):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                for index, prior in enumerate(backlog):
                    if predicate(prior):
                        return backlog.pop(index)
                if fixture_executor is None:
                    rclpy.spin_once(node, timeout_sec=0.01)
                try:
                    event = events.get(timeout=0.01)
                except queue.Empty:
                    continue
                if predicate(event):
                    return event
                if event.get('event') in ['invalid', 'error']:
                    raise AssertionError(event)
                backlog.append(event)
                if len(backlog) > 256:
                    backlog.pop(0)
            raise AssertionError('Timed out waiting for backend event')
        ready = wait(lambda e: e.get('event') == 'ready')
        assert ready['version'] == VERSION, ready
        counter = 0
        def send(operation):
            nonlocal counter
            counter += 1
            command = dict(command='tool', generation=1, request=counter, subscription=counter, operation=operation)
            process.stdin.write(json.dumps(command) + '\n')
            process.stdin.flush()
            return counter
        def response(operation, seconds=10):
            identity = send(operation)
            return wait(lambda e: e.get('event') == 'tool' and e['request'] == identity, seconds)['update']
        def check(name, action):
            result = action()
            assert result, name
            report['cases'].append(dict(name=name, outcome='passed'))
        def discovered(section, name):
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                if name in response(dict(kind='graph', section=section, node=''))['value']:
                    return True
                time.sleep(0.1)
            return False
        time.sleep(1)
        if args.stage in ['topics', 'all']:
            check('topic graph', lambda: discovered('Topics', '/studio_fixture/tools_text'))
            check('native schema', lambda: response(dict(kind='schema', message_type='std_msgs/msg/String', bundle=''))['value']['default'] == {'data': ''})
            check('endpoint QoS', lambda: len(response(dict(kind='details', topic='/studio_fixture/tools_text'))['value']['subscribers']) >= 1)
            invalid = response(dict(kind='publish', topic='/studio_fixture/tools_text', message_type='std_msgs/msg/String', bundle='', value={'data': 4}, interval_ms=0, best_effort=False, transient_local=False))
            check('invalid typed payload rejected', lambda: invalid['state'] == 'error')
            publisher = send(dict(kind='publish', topic='/studio_fixture/tools_text', message_type='std_msgs/msg/String', bundle='', value={'data': 'tools-native'}, interval_ms=100, best_effort=False, transient_local=False))
            check('publisher started', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == publisher)['update']['state'] == 'publishing')
            deadline = time.monotonic() + 8
            while not received and time.monotonic() < deadline:
                if fixture_executor is None:
                    rclpy.spin_once(node, timeout_sec=0.05)
            check('actual ROS publication', lambda: 'tools-native' in received)
            statistics = send(dict(kind='statistics', topic='/studio_fixture/tools_text', message_type='std_msgs/msg/String', best_effort=False, transient_local=False))
            check('received statistics', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == statistics and e['update']['state'] == 'statistics' and e['update']['value']['received_samples'] > 0)['update']['value']['serialized_bytes_per_second'] > 0)
            check('stop statistics', lambda: response(dict(kind='stop', target=statistics))['state'] == 'stopped')
            check('stop publisher', lambda: response(dict(kind='stop', target=publisher))['state'] == 'stopped')
            once_id = send(dict(kind='publish', topic='/studio_fixture/tools_text', message_type='std_msgs/msg/String', bundle='', value={'data': 'one-shot-native'}, interval_ms=0, best_effort=False, transient_local=False))
            result = wait(lambda e: e.get('event') == 'tool' and e['request'] == once_id and e['update']['terminal'])['update']
            deadline = time.monotonic() + 3
            while 'one-shot-native' not in received and time.monotonic() < deadline:
                if fixture_executor is None:
                    rclpy.spin_once(node, timeout_sec=0.05)
                else:
                    time.sleep(0.01)
            check('publish once actual delivery', lambda: result['state'] == 'published' and received.count('one-shot-native') == 1)
        if args.stage in ['services', 'all']:
            check('service graph', lambda: discovered('Services', '/studio_fixture/tools_add'))
            check('service schemas', lambda: response(dict(kind='schema', message_type='example_interfaces/srv/AddTwoInts', bundle=''))['value']['default'] == {'a': 0, 'b': 0})
            check('service providers', lambda: any(p['name'] == 'studio_tools_fixture' and p['namespace'] == '/studio_fixture' for p in response(dict(kind='service_info', service='/studio_fixture/tools_add'))['value']['providers']))
            def call(service, value, timeout=10000):
                return send(dict(kind='call', service=service, message_type='example_interfaces/srv/AddTwoInts', bundle='', value=value, timeout_ms=timeout))
            identity = call('/studio_fixture/tools_add', {'a': 18446744073709551615, 'b': 0})
            check('request range rejection', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == identity)['update']['state'] == 'error')
            identity = call('/studio_fixture/tools_add', {'a': 40, 'b': 2})
            check('actual ROS service response', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'])['update']['value']['response']['sum'] == 42)
            identity = call('/studio_fixture/missing', {'a': 1, 'b': 2}, 100)
            check('unavailable timeout not sent', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'])['update']['value']['request_sent'] is False)
            identity = call('/studio_fixture/tools_add', {'a': -99, 'b': 2}, 6000)
            check('sent timeout distinct', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'])['update']['value']['request_sent'] is True)
            identity = call('/studio_fixture/missing', {'a': 1, 'b': 2})
            wait(lambda e: e.get('event') == 'tool' and e['request'] == identity)
            check('stop waiting', lambda: response(dict(kind='stop', target=identity))['state'] == 'stopped')
        if args.stage == 'nav2':
            schemas = {}
            for name in ['NavigateToPose', 'NavigateThroughPoses', 'ComputePathToPose']:
                schema = response(dict(kind='schema', message_type='nav2_msgs/action/' + name, bundle=''))
                check('bundled Nav2 ' + name + ' schema', lambda schema=schema: schema['state'] == 'schema' and isinstance(schema['value']['default'], dict))
                schemas[name] = schema['value']['default']
            check('synthetic Nav2 action discovery', lambda: discovered('Actions', '/studio_fixture/tools_nav2'))
            def nav_goal(x):
                value = json.loads(json.dumps(schemas['NavigateToPose']))
                value['pose']['header']['frame_id'] = 'map'
                value['pose']['pose']['position']['x'] = float(x)
                value['pose']['pose']['orientation']['w'] = 1.0
                return send(dict(kind='goal', action='/studio_fixture/tools_nav2', message_type='nav2_msgs/action/NavigateToPose', bundle='', value=value, timeout_ms=10000))
            def nav_terminal(identity):
                return wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'])['update']
            identity = nav_goal(-1)
            check('Nav2 goal rejection', lambda: nav_terminal(identity)['state'] == 'goal rejected')
            identity = nav_goal(1)
            check('Nav2 feedback', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['state'] == 'feedback'))
            check('Nav2 result', lambda: nav_terminal(identity)['state'] == 'succeeded')
            identity = nav_goal(100)
            wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['state'] == 'goal accepted')
            cancel = send(dict(kind='cancel', target=identity))
            check('Nav2 cancellation acknowledged', lambda: nav_terminal(cancel)['state'] == 'cancel acknowledged')
            check('Nav2 cancelled result', lambda: nav_terminal(identity)['state'] == 'cancelled')
            check('foreign Nav2 cancellation refused', lambda: response(dict(kind='cancel', target=999999))['state'] == 'error')
            before = node.tools_goal_requests
            process.stdin.close()
            process.wait(timeout=5)
            time.sleep(.1)
            check('Nav2 disconnect adds no operation', lambda: node.tools_goal_requests == before and process.returncode == 0)
        if args.stage in ['actions', 'all']:
            check('action graph', lambda: discovered('Actions', '/studio_fixture/tools_fibonacci'))
            check('goal schema', lambda: response(dict(kind='schema', message_type='example_interfaces/action/Fibonacci', bundle=''))['value']['default'] == {'order': 0})
            def goal(order):
                return send(dict(kind='goal', action='/studio_fixture/tools_fibonacci', message_type='example_interfaces/action/Fibonacci', bundle='', value={'order': order}, timeout_ms=10000))
            def terminal(identity):
                return wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'])['update']
            identity = goal(-1)
            check('goal rejection', lambda: terminal(identity)['state'] == 'goal rejected')
            identity = goal(5)
            check('feedback', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['state'] == 'feedback')['update']['value']['feedback']['sequence'])
            check('goal success', lambda: terminal(identity)['state'] == 'succeeded')
            identity = goal(99)
            check('goal abort', lambda: terminal(identity)['state'] == 'aborted')
            identity = goal(50)
            check('goal accepted', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['state'] == 'goal accepted'))
            cancel = send(dict(kind='cancel', target=identity))
            check('cancel acknowledged', lambda: terminal(cancel)['state'] == 'cancel acknowledged')
            check('cancel terminal result', lambda: terminal(identity)['state'] == 'cancelled')
            check('foreign goal cancellation refused', lambda: response(dict(kind='cancel', target=999999))['state'] == 'error')
        if args.stage in ['parameters', 'all']:
            target = '/studio_fixture/studio_tools_fixture'
            check('node graph', lambda: discovered('Nodes', target))
            check('node relationships', lambda: '/studio_fixture/tools_text' in response(dict(kind='node_info', node=target))['value']['subscribers'])
            check('parameter node discovery', lambda: discovered('Parameters', target))
            def parameters(operation):
                identity = send(operation)
                return wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'], 20)['update']
            def read(names):
                return parameters(dict(kind='parameter_read', node=target, names=names, timeout_ms=15000))
            def write(changes, atomic=True):
                return parameters(dict(kind='parameter_write', node=target, changes=changes, atomic=atomic, timeout_ms=15000))
            result = read([])
            check('list values and constraints', lambda: result['state'] == 'parameters' and 'speed' in result['value']['names'] and len(result['value']['descriptors']) == len(result['value']['names']))
            check('all scalar and array parameter types', lambda: set(range(1, 10)).issubset({v['type'] for v in result['value']['values']}))
            events_id = send(dict(kind='parameter_events', node=target))
            check('explicit parameter events', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == events_id)['update']['state'] == 'parameter events started')
            result = write([dict(name='speed', parameter_type=2, value=4)])
            check('write and actual readback', lambda: result['value']['values'][0]['integer_value'] == 4 and result['value']['write_results']['result']['successful'])
            check('filtered parameter event', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == events_id and e['update']['state'] == 'parameter event' and any(p['name'] == 'speed' for p in e['update']['value']['event']['changed_parameters'])))
            result = write([dict(name='serial', parameter_type=4, value='changed')])
            check('read-only rejection', lambda: result['value']['write_results']['result']['successful'] is False and result['value']['values'][0]['string_value'] == 'fixture-only')
            result = write([dict(name='speed', parameter_type=2, value=6), dict(name='label', parameter_type=4, value='reject')])
            check('atomic rejection preserves both', lambda: result['value']['write_results']['result']['successful'] is False and result['value']['values'][0]['integer_value'] == 4)
            result = write([dict(name='speed', parameter_type=2, value=6), dict(name='label', parameter_type=4, value='reject')], False)
            check('non-atomic partial result', lambda: result['value']['write_results']['results'][0]['successful'] and result['value']['write_results']['results'][1]['successful'] is False and result['value']['values'][0]['integer_value'] == 6)
            check('stop parameter events', lambda: response(dict(kind='stop', target=events_id))['state'] == 'stopped')
        if args.custom_bundle:
            bundle = str(args.custom_bundle.resolve())
            check('custom topic discovery', lambda: discovered('Topics', '/studio_fixture/tools_custom'))
            check('validated custom schema import', lambda: response(dict(kind='schema', message_type='feasibility_interfaces/msg/Sample', bundle=bundle))['state'] == 'schema')
            payload = dict(sequence=18446744073709551615, nested=dict(label='custom-exact', value=1.25), fixed=[1.0, 2.0, 3.0], bounded=[-1, 2], labels=['one', 'two'], payload=[1, 255])
            publisher = send(dict(kind='publish', topic='/studio_fixture/tools_custom', message_type='feasibility_interfaces/msg/Sample', bundle=bundle, value=payload, interval_ms=100, best_effort=False, transient_local=False))
            check('custom publisher started', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == publisher)['update']['state'] == 'publishing')
            until = time.monotonic() + 8
            while not custom_received and time.monotonic() < until:
                time.sleep(0.02)
            check('actual custom publication preserves uint64', lambda: custom_received and custom_received[-1] == 18446744073709551615)
            counter += 1
            process.stdin.write(json.dumps(dict(command='start', generation=1, request=counter, subscription=50000, configuration=dict(topic='/studio_fixture/tools_custom', message_type='feasibility_interfaces/msg/Sample', best_effort=False, transient_local=False, output='typed'))) + '\n')
            process.stdin.flush()
            check('custom inspection subscription', lambda: wait(lambda e: e.get('event') == 'started' and e['subscription'] == 50000))
            sample = wait(lambda e: e.get('event') == 'observation' and e['subscription'] == 50000)
            check('custom exact typed inspection', lambda: any(f['name'] == 'sequence' and f['value']['value'] == '18446744073709551615' for f in sample['sample']['value']['fields']))
            counter += 1
            process.stdin.write(json.dumps(dict(command='stop', generation=1, request=counter, subscription=50000)) + '\n')
            process.stdin.flush()
            check('custom inspection stop', lambda: wait(lambda e: e.get('event') == 'stopped' and e['subscription'] == 50000))
            check('custom publisher stop', lambda: response(dict(kind='stop', target=publisher))['state'] == 'stopped')
            identity = send(dict(kind='call', service='/studio_fixture/tools_inspect', message_type='feasibility_interfaces/srv/Inspect', bundle=bundle, value={'sample': payload}, timeout_ms=10000))
            result = wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'])['update']
            check('actual custom service', lambda: result['value']['response']['accepted'] and result['value']['response']['reason'] == 'custom-exact')
            check('custom goal schema', lambda: response(dict(kind='schema', message_type='feasibility_interfaces/action/Count', bundle=bundle))['value']['default'] == {'target': 0})
            identity = send(dict(kind='goal', action='/studio_fixture/tools_count', message_type='feasibility_interfaces/action/Count', bundle=bundle, value={'target': 3}, timeout_ms=10000))
            result = wait(lambda e: e.get('event') == 'tool' and e['request'] == identity and e['update']['terminal'])['update']
            check('actual custom action result', lambda: result['state'] == 'succeeded' and result['value']['result']['completed'] == 3)
            check('missing bundle refused', lambda: response(dict(kind='schema', message_type='feasibility_interfaces/msg/Sample', bundle=str(evidence/'missing')))['state'] == 'error')
        if args.stage == 'all':
            publisher = send(dict(kind='publish', topic='/studio_fixture/tools_text', message_type='std_msgs/msg/String', bundle='', value={'data': 'concurrent'}, interval_ms=50, best_effort=False, transient_local=False))
            wait(lambda e: e.get('event') == 'tool' and e['request'] == publisher)
            goal_id = send(dict(kind='goal', action='/studio_fixture/tools_fibonacci', message_type='example_interfaces/action/Fibonacci', bundle='', value={'order': 100}, timeout_ms=10000))
            wait(lambda e: e.get('event') == 'tool' and e['request'] == goal_id and e['update']['state'] == 'goal accepted')
            call_id = send(dict(kind='call', service='/studio_fixture/tools_add', message_type='example_interfaces/srv/AddTwoInts', bundle='', value={'a': 2, 'b': 3}, timeout_ms=10000))
            result = wait(lambda e: e.get('event') == 'tool' and e['request'] == call_id and e['update']['terminal'])['update']
            check('concurrent service completes beside publisher and goal', lambda: result['value']['response']['sum'] == 5 and 'concurrent' in received)
            cancel_id = send(dict(kind='cancel', target=goal_id))
            wait(lambda e: e.get('event') == 'tool' and e['request'] == cancel_id and e['update']['terminal'])
            wait(lambda e: e.get('event') == 'tool' and e['request'] == goal_id and e['update']['terminal'])
            response(dict(kind='stop', target=publisher))
            process.stdin.write(json.dumps(dict(command='tool', generation=1, request=publisher, subscription=publisher, operation=dict(kind='publish', topic='/studio_fixture/tools_text', message_type='std_msgs/msg/String', bundle='', value={'data': 'stale-replay'}, interval_ms=0, best_effort=False, transient_local=False))) + '\n')
            process.stdin.flush()
            check('stale operation refused without replay', lambda: wait(lambda e: e.get('event') == 'tool' and e['request'] == publisher)['update']['state'] == 'error')
            pending = []
            for _ in range(16):
                identity = send(dict(kind='call', service='/studio_fixture/no_server', message_type='example_interfaces/srv/AddTwoInts', bundle='', value={'a': 1, 'b': 2}, timeout_ms=60000))
                update = wait(lambda e: e.get('event') == 'tool' and e['request'] == identity)['update']
                assert update['state'] == 'waiting for service', update
                pending.append(identity)
            check('global operation capacity enforced', lambda: response(dict(kind='call', service='/studio_fixture/no_server', message_type='example_interfaces/srv/AddTwoInts', bundle='', value={'a': 1, 'b': 2}, timeout_ms=60000))['state'] == 'error')
            for identity in pending:
                assert response(dict(kind='stop', target=identity))['state'] == 'stopped'
            check('all admitted operations stop independently', lambda: True)
        if args.stage == 'all':
            live_publisher = send(dict(kind='publish', topic='/studio_fixture/tools_text', message_type='std_msgs/msg/String', bundle='', value={'data': 'before-disconnect'}, interval_ms=50, best_effort=False, transient_local=False))
            wait(lambda e: e.get('event') == 'tool' and e['request'] == live_publisher)
            live_goal = send(dict(kind='goal', action='/studio_fixture/tools_fibonacci', message_type='example_interfaces/action/Fibonacci', bundle='', value={'order': 100}, timeout_ms=10000))
            wait(lambda e: e.get('event') == 'tool' and e['request'] == live_goal and e['update']['state'] == 'goal accepted')
            cancellations = node.tools_cancel_requests
            process.stdin.close()
            process.wait(timeout=5)
            reader.join(timeout=2)
            report['first_backend_exit'] = process.returncode
            check('EOF cleans active backend endpoints', lambda: process.returncode == 0 and not reader.is_alive())
            time.sleep(1)
            published_before = len(received)
            goals_before = node.tools_goal_requests
            events = queue.Queue()
            backlog.clear()
            process = subprocess.Popen([str(ROOT / 'target/debug/studio-backend'), '--observe-session'], cwd=ROOT, env=backend_env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True)
            reader = threading.Thread(target=read_protocol, daemon=True)
            reader.start()
            wait(lambda e: e.get('event') == 'ready')
            wait(lambda e: e.get('event') == 'graph')
            time.sleep(0.5)
            check('new backend remains inactive without publisher or goal replay', lambda: len(received) == published_before and node.tools_goal_requests == goals_before)
            check('disconnect sends no hidden action cancellation', lambda: node.tools_cancel_requests == cancellations)
        assert report['cases'], 'No acceptance cases executed for this stage'
        report['outcome'] = 'passed'
    except Exception as error:
        report['error'] = repr(error)
    finally:
        if process:
            if not process.stdin.closed:
                process.stdin.close()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                report['outcome'] = 'failed'
                report['cleanup_error'] = 'Backend did not exit after stdin EOF'
            report['backend_exit'] = process.returncode
            report['backend_reaped'] = process.poll() is not None
            reader.join(timeout=2)
            (evidence / 'protocol.jsonl').write_text(''.join(transcript))
        if log:
            log.close()
        if fixture_executor:
            fixture_executor.shutdown(timeout_sec=5)
            fixture_thread.join(timeout=5)
        if action_server:
            action_server.destroy()
        if custom_handles:
            custom_handles[-1].destroy()
        if node:
            node.destroy_node()
            rclpy.shutdown()
        report_path = ROOT / 'artifacts/feasibility' / (args.label + '.json')
        report_path.write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    return 0 if report['outcome'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
