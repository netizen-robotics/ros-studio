"""Local synthetic service server; never connects to the robot domain."""
import argparse
import json
import os
from pathlib import Path
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--kind', choices=['inspect', 'reflect'], required=True)
    parser.add_argument('--mode', choices=['normal', 'reject', 'delay', 'large'], default='normal')
    parser.add_argument('--log', type=Path, required=True)
    args = parser.parse_args()
    if os.environ.get('ROS_DOMAIN_ID') != '187' or os.environ.get('ROS_LOCALHOST_ONLY') != '1':
        raise SystemExit('Fixture server requires localhost/domain 187')
    import rclpy
    from rclpy.executors import ExternalShutdownException
    from rosidl_runtime_py.convert import message_to_ordereddict
    if args.kind == 'inspect':
        from feasibility_interfaces.srv import Inspect as Service
    else:
        from service_extra_interfaces.srv import Reflect as Service
        from service_extra_interfaces.msg import Entry
    rclpy.init()
    node = rclpy.create_node('studio_service_fixture')
    def callback(request, response):
        # Flushed before responding/delaying: observable at-most-once receipt evidence.
        with args.log.open('a') as stream:
            stream.write(json.dumps(message_to_ordereddict(request)) + '\n')
        if args.mode == 'delay':
            time.sleep(8)
        if args.kind == 'inspect':
            response.accepted = args.mode != 'reject'
            response.reason = 'x' * (1024 * 1024 + 1) if args.mode == 'large' else ('fixture rejection' if args.mode == 'reject' else 'validated')
        else:
            for name in ['enabled', 'small', 'large', 'weight', 'values', 'flags', 'tags']:
                setattr(response, name, getattr(request, name))
            response.summary = Entry(label=request.title, value=float(request.weight))
            response.entries = [Entry(label=tag, value=float(i)) for i, tag in enumerate(request.tags)]
        return response
    service = node.create_service(Service, '/studio_fixture/' + args.kind, callback)
    print('READY', flush=True)
    try:
        rclpy.spin(node)
    except (ExternalShutdownException, KeyboardInterrupt):
        pass
    finally:
        node.destroy_service(service)
        node.destroy_node()
        rclpy.try_shutdown()


if __name__ == '__main__':
    main()
